//! `ax`: aio-rs's command (aio: `am`). Read-only views into an app's store work
//! while the app runs (SQLite WAL readers never block its writer).
mod add;
mod agent;
/// APKs: made by the NDK and Gradle (Linux, macOS, Windows).
mod android;
mod auth;
mod boxlab;
mod build;
mod bundle;
mod cdp;
mod check;
mod components;
mod control;
mod create;
mod css;
mod dev;
mod display;
mod doctor;
mod eval;
mod fetched;
mod fleet;
mod freeze;
mod headless;
mod hotpatch;
mod icon;
/// `ax install --android`: next to the APK build.
mod install;
mod installed;
mod ios;
mod lab;
mod lint;
mod live;
mod logs;
mod manifest;
mod metrics;
mod mux;
/// `.deb` and AppImage: made on Linux (dpkg-deb, mksquashfs).
#[cfg(unix)]
mod package;
mod pin;
mod places;
mod platform;
mod prebuilt;
mod preview;
mod prune;
mod record;
mod release;
mod reload;
mod remote;
mod replayed;
mod runs;
mod service;
mod setup;
mod shape;
mod shot;
mod sizes;
mod smoke;
mod splash;
mod sql;
mod start;
mod status;
mod stop;
mod surface;
mod target;
mod testgen;
mod testrun;
mod theme;
mod travel;
mod trust;
mod tsgen;
mod ui;
mod upgrade;
mod verbs;
mod version;
mod video;
mod watch;
mod web;
mod zig;

use rusqlite::{Connection, OpenFlags, params};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use target::Target;

type Out = Result<String, String>;

/// The last `n` lines across the rotated files, oldest first.
fn log_lines(dir: &Path, n: usize) -> Out {
    let mut lines: Vec<String> = Vec::new();
    for i in 0..5 {
        let name = if i == 0 { "app.log".to_string() } else { format!("app.{i}.log") };
        let Ok(text) = std::fs::read_to_string(dir.join(name)) else { break };
        let mut older: Vec<String> = text.lines().map(String::from).collect();
        older.append(&mut lines);
        lines = older;
        if lines.len() >= n {
            break;
        }
    }
    if lines.is_empty() {
        return Err(format!("no logs in {}", dir.display()));
    }
    Ok(lines[lines.len().saturating_sub(n)..].join("\n"))
}

/// The newest `n` warnings and errors in the log lines `lines` (oldest
/// first), each under the code generation it came from (pure): each start
/// or hot-patch (its number, 0 before the first; its line) with its bad
/// lines, and how many there were in all (aio's log generation per reload).
/// A start (its number, its line) and the bad lines after it.
type Run = (usize, String, Vec<String>);

fn bad_lines(lines: &[&str], n: usize) -> (Vec<Run>, usize) {
    // (start, its line, the bad events after it: a panic's lines are one)
    let mut runs: Vec<Run> = vec![(0, String::new(), vec![])];
    let mut in_bad = false;
    for l in lines {
        if starts(l) || patched(l) {
            runs.push((runs.len(), l.to_string(), vec![]));
            in_bad = false;
        } else if bad(l)
            && let Some(r) = runs.last_mut()
        {
            r.2.push(l.to_string());
            in_bad = true;
        } else if in_bad && logs::continues(l) {
            if let Some(e) = runs.last_mut().and_then(|r| r.2.last_mut()) {
                e.push('\n');
                e.push_str(l);
            }
        } else {
            in_bad = false;
        }
    }
    let total: usize = runs.iter().map(|r| r.2.len()).sum();
    let mut skip = total.saturating_sub(n);
    let mut out = vec![];
    for (k, start, bad) in runs {
        let take = bad.len().saturating_sub(skip);
        skip = skip.saturating_sub(bad.len());
        if take > 0 {
            out.push((k, start, bad[bad.len() - take..].to_vec()));
        }
    }
    (out, total)
}

/// A log line of an app's start (`[aio] notes 2.0.0 starting (pid 10)`).
fn starts(l: &str) -> bool {
    let rest = l.split_once(' ').map_or("", |(_, r)| r);
    rest.starts_with("[aio] ") && rest.contains(" starting (pid ")
}

/// A log line of a hot-patch applied (`aio::log::hot_patched`): new code.
fn patched(l: &str) -> bool {
    let rest = l.split_once(' ').map_or("", |(_, r)| r);
    rest.starts_with("[aio] hot-patch ") && rest.contains(" applied")
}

/// A warning, an error or a panic.
fn bad(l: &str) -> bool {
    let rest = l.split_once(' ').map_or("", |(_, r)| r);
    rest.starts_with("WARN ") || rest.starts_with("ERROR ") || rest.contains(" panicked at ")
}

/// `lines` from the first one stamped at or after `since` (pure; a log is
/// written in order). `reload`: since the newest start, or the hot patch
/// at `patched` after it.
fn newer<'a>(lines: &'a [&'a str], since: Option<i64>, reload: Option<Option<i64>>) -> &'a [&'a str] {
    let mut from = 0;
    if let Some(patched) = reload {
        from = lines.iter().rposition(|l| starts(l)).unwrap_or(0);
        let start = logs::head(lines.get(from).copied().unwrap_or_default()).map_or(0, |h| h.at);
        if let Some(p) = patched.filter(|p| *p > start) {
            from = lines[from..]
                .iter()
                .position(|l| logs::head(l).is_some_and(|h| h.at >= p))
                .map_or(lines.len(), |i| from + i);
        }
    }
    if let Some(s) = since {
        from = lines[from..]
            .iter()
            .position(|l| logs::head(l).is_some_and(|h| h.at >= s))
            .map_or(lines.len(), |i| from + i);
    }
    &lines[from..]
}

/// `ax errors`: the failed build `build` (`ax dev`'s `build.log`) first,
/// then [`bad_lines`], as text or JSON (pure).
fn errors(build: Option<&str>, lines: &[&str], n: usize, json: bool) -> String {
    let (runs, total) = bad_lines(lines, n);
    if json {
        let runs: Vec<serde_json::Value> = runs
            .iter()
            .map(|(k, start, bad)| serde_json::json!({ "start": k, "line": (*k > 0).then_some(start), "lines": bad }))
            .collect();
        return serde_json::json!({ "build": build, "starts": runs, "total": total, "read": lines.len() }).to_string();
    }
    let mut out: Vec<String> = Vec::new();
    if let Some(b) = build {
        out.push(format!("{} (the app runs its last good build)", b.lines().next().unwrap_or("build failed")));
        out.extend(b.lines().skip(1).take(60).map(|l| format!("  {l}")));
    }
    for (k, start, bad) in runs {
        out.push(match k {
            0 => "before the first start in the log:".into(),
            _ if patched(&start) => format!("patch {k}: {start}"),
            _ => format!("start {k}: {start}"),
        });
        out.extend(bad.iter().map(|l| format!("  {}", l.replace('\n', "\n  "))));
    }
    if total == 0 {
        out.push(format!("no warnings or errors in the log ({} lines)", lines.len()));
    }
    out.join("\n")
}

/// Open a store read-only, refusing files that aren't aio-rs stores.
fn open(path: &Path) -> Result<Connection, String> {
    if !path.exists() {
        return Err(format!("no store at {}", path.display()));
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())?;
    let v = aio::update::local_data(path).ok().flatten().map_or(0, |c| c.schema);
    if !(1..=2).contains(&v) {
        return Err(format!("{} is not an aio-rs store (schema {v})", path.display()));
    }
    Ok(db)
}

/// The changes event `seq` made (`ax events --diff`), none without it.
type Diff<'a> = dyn Fn(i64) -> Vec<(String, serde_json::Value, serde_json::Value)> + 'a;

/// The newest `last` events (of `cell`), oldest first, each with the
/// changes `diff` says it made.
fn events(db: &Connection, cell: Option<&str>, last: u32, json: bool, diff: &Diff) -> Out {
    let mut q = db
        .prepare(
            "SELECT seq, cell, method, args, by, at FROM (SELECT * FROM log WHERE ?1 IS NULL OR cell = ?1
               ORDER BY seq DESC LIMIT ?2) ORDER BY seq",
        )
        .map_err(|e| e.to_string())?;
    let rows = q
        .query_map(params![cell, last], |r| {
            let (seq, cell, method, args, by, at): (i64, String, String, String, String, i64) =
                (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?);
            let args: serde_json::Value = serde_json::from_str(&args).unwrap_or(serde_json::Value::String(args));
            // A `#[redact]` call: what is logged is the state it made, never shown here.
            let args = match (args.get(aio::REDACTED).is_some(), args.get(aio::TX)) {
                (true, _) => serde_json::Value::String("[redacted]".into()),
                // A transaction: logged with the state it left; its call's arguments shown.
                (false, Some(_)) => args["args"].clone(),
                (false, None) => args,
            };
            let changes = diff(seq);
            Ok(match json {
                true => {
                    let mut row = serde_json::json!({ "seq": seq, "at": at, "cell": cell, "method": method, "args": args, "by": by });
                    if !changes.is_empty() {
                        row["diff"] = changes
                            .iter()
                            .map(|(p, b, a)| serde_json::json!({ "path": p, "before": b, "after": a }))
                            .collect();
                    }
                    row.to_string()
                }
                false => {
                    let args = args.as_str().map(String::from).unwrap_or_else(|| args.to_string());
                    let head = format!("{seq:>6}  {at}  {cell}.{method}({})  by {by}", short(&args, 120));
                    std::iter::once(head).chain(changes.iter().map(|c| format!("          {}", travel::said(c)))).collect::<Vec<_>>().join("\n")
                }
            })
        })
        .map_err(|e| e.to_string())?;
    let lines: Vec<String> = rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    Ok(match json {
        true => format!("[{}]", lines.join(",")),
        false if lines.is_empty() => "no events".into(),
        false => lines.join("\n"),
    })
}

/// `ax events --json`'s rows with what the running app read of them
/// (`read`: each cell's `took` answer): each event's `ms` when it ran in
/// this run, and with `refused` its refused calls as rows of their own
/// (`{at, cell, method, ms, refused}`, no seq), all by time (pure).
fn timed(rows: &mut Vec<serde_json::Value>, read: &[(String, serde_json::Value)], refused: bool) {
    for r in rows.iter_mut() {
        let at = read.iter().find(|(c, _)| r["cell"] == c.as_str());
        if let Some(ms) = at.and_then(|(_, v)| v["ms"].get(r["seq"].to_string())).filter(|m| m.is_number()) {
            r["ms"] = ms.clone();
        }
    }
    if !refused {
        return;
    }
    for (cell, v) in read {
        for e in v["refused"].as_array().into_iter().flatten() {
            let method = e["call"].as_str().and_then(|c| c.strip_prefix(cell.as_str())?.strip_prefix('.'));
            let at = e["at"].as_str().and_then(|a| chrono::DateTime::parse_from_rfc3339(a).ok());
            if let (Some(method), Some(at)) = (method, at) {
                rows.push(serde_json::json!({
                    "at": at.timestamp_millis(), "cell": cell, "method": method, "ms": e["ms"], "refused": e["refused"],
                }));
            }
        }
    }
    rows.sort_by_key(|r| r["at"].as_i64().unwrap_or(0));
}

fn short(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}… ({} bytes)", &s[..i], s.len()),
        None => s.to_string(),
    }
}

/// The newest stored snapshot of `cell`: what it says, and the state.
fn stored(db: &Connection, cell: &str) -> Result<(String, serde_json::Value), String> {
    let snap: Option<(i64, i64, String)> = db
        .query_row("SELECT seq, v, state FROM snap WHERE cell = ?1 ORDER BY seq DESC LIMIT 1", [cell], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .ok();
    let Some((seq, v, text)) = snap else { return Err(format!("no snapshot of `{cell}` yet")) };
    let newer: i64 = db
        .query_row("SELECT COUNT(*) FROM log WHERE cell = ?1 AND seq > ?2", params![cell, seq], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let st = serde_json::from_str(&text).map_err(|e| format!("snapshot {seq} of `{cell}`: {e}"))?;
    let note = if newer > 0 { format!(" ({newer} newer events not in it yet)") } else { String::new() };
    Ok((format!("{cell} v{v} after event {seq}{note}"), st))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// App `app`'s library (the package in the folder of its aio.toml) and that folder.
/// A call's arguments: `--body` (the one object), else the words or `--args`.
fn call_body(words: &[&str], args: Option<&str>, body: Option<&str>) -> Result<serde_json::Value, String> {
    match body {
        Some(_) if !words.is_empty() || args.is_some() => {
            Err("--body is the one argument: no words or --args with it".into())
        }
        Some(b) => live::body(b),
        None => live::call_args(words, args),
    }
}

fn app_lib(app: &str) -> Result<(String, PathBuf), String> {
    let meta = dev::metadata()?;
    let pkg = dev::locate(&meta, app)?;
    let dir = build::app_dir(&pkg)?;
    let lib = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["manifest_path"].as_str().is_some_and(|m| Path::new(m).parent() == Some(dir.as_path())))
        .and_then(|p| p["name"].as_str())
        .ok_or(format!("{app}: no package beside its aio.toml"))?;
    Ok((lib.to_string(), dir))
}

/// The states right after each event `ask` names (by cell; each with the
/// event before it, for diffs): the running app's own, over its door,
/// unless `other` (a store that isn't its, `--from`) or it isn't running;
/// else the store `file` replayed by the app's code (`replayed`).
fn states_of(
    t: &Target,
    file: &Path,
    other: bool,
    ask: &std::collections::BTreeMap<String, Vec<u64>>,
) -> Result<replayed::States, String> {
    if !other && aio::paths::running(&t.plan.data) {
        let mut out = replayed::States::new();
        for (cell, seqs) in ask {
            out.insert(cell.clone(), travel::states(&door(t, cell)?, cell, &user(None), seqs)?);
        }
        return Ok(out);
    }
    let (lib, dir) = app_lib(&t.name).map_err(|e| {
        format!(
            "{e}\n(a stopped app's or another store's states are replayed by the app's code: run this in its workspace)"
        )
    })?;
    replayed::states(&lib, &dir, file, ask)
}

/// The version every build of this run says (pure): `--app-version`, else
/// `AIORT_BUILD_VERSION`, else aio's `AIO_BUILD_VERSION`.
fn build_version(flag: Option<&str>, ours: Option<String>, aios: Option<String>) -> Option<String> {
    flag.map(String::from).or(ours).or(aios)
}

/// `ax call --wait <s>`: its answer waited for that long at most.
fn call_wait(s: Option<&str>) -> Result<Option<std::time::Duration>, String> {
    s.map(|s| {
        s.parse::<f64>()
            .ok()
            .filter(|s| *s > 0.0 && s.is_finite())
            .map(std::time::Duration::from_secs_f64)
            .ok_or(format!("--wait {s}: seconds, more than 0"))
    })
    .transpose()
}

/// Who the verbs that reach the running app are: `--as`, else this OS user.
fn user(as_user: Option<&str>) -> String {
    as_user.map(String::from).unwrap_or_else(|| {
        std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "user".into())
    })
}

/// The door of the running app `t`'s cell; `Err` (nothing sent) when the
/// app isn't running.
fn door(t: &Target, cell: &str) -> Result<aio::sync::Link, String> {
    if !aio::paths::running(&t.plan.data) {
        return Err(format!("{} isn't running (nothing was sent)", t.name));
    }
    let addr = aio::sync::door(&t.id, &t.plan.data, cell).map_err(|e| e.to_string())?;
    Ok(aio::sync::Link { addr, token: String::new(), root: None })
}

/// The cells whose doors the running app `t` has open (its socket folder;
/// a Windows pipe's name), and those its store holds, sorted.
fn doors(t: &Target) -> Vec<String> {
    let mut cells: Vec<String> = aio::door::open(&t.id, &t.plan.data)
        .into_iter()
        .chain(open(&t.store()).and_then(|db| metrics::cells(&db)).unwrap_or_default())
        .collect();
    cells.sort();
    cells.dedup();
    cells
}

/// Live from the running app; else the stored snapshot, saying why.
fn state(t: &Target, cell: &str, path: Option<&str>, as_user: Option<&str>) -> Out {
    let away = match door(t, cell) {
        Ok(link) => match live::state(&link, cell, path, &user(as_user)) {
            Ok(out) => return out,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => {
                format!("the running app has no door for `{cell}`")
            }
            Err(e) => return Err(e.to_string()),
        },
        Err(_) => "the app isn't running".into(),
    };
    let (head, st) = stored(&open(&t.store())?, cell)?;
    live::show(&format!("{head} (stored: {away})"), &st, path)
}

/// Every cell of the store as one JSON object: `live`'s view of a cell
/// when it has one, else its newest stored snapshot (`null`: none yet).
fn dump(db: &Connection, live: &dyn Fn(&str) -> Option<serde_json::Value>) -> Out {
    let mut all = serde_json::Map::new();
    for cell in metrics::cells(db)? {
        let v = live(&cell).or_else(|| stored(db, &cell).ok().map(|(_, v)| v)).unwrap_or_default();
        all.insert(cell, v);
    }
    serde_json::to_string_pretty(&all).map_err(|e| e.to_string())
}

/// `ax snapshot <app>`: [`dump`], live (as `user` sees it) when the app runs.
fn snapshot(t: &Target, as_user: Option<&str>) -> Out {
    let live = |cell: &str| {
        let link = door(t, cell).ok()?;
        aio::rt().block_on(aio::sync::Live::connect(&link, cell, &user(as_user))).ok().map(|l| l.view().clone())
    };
    dump(&open(&t.store())?, &live)
}

/// `ax snapshot save <app> [file]`: the snapshot as a file (a new one).
fn snapshot_save(t: &Target, file: Option<&str>, as_user: Option<&str>) -> Out {
    let file = PathBuf::from(file.map_or_else(|| format!("{}-snapshot.json", t.id), String::from));
    if file.exists() {
        return Err(format!("{} exists: pick a new name", file.display()));
    }
    std::fs::write(&file, snapshot(t, as_user)?).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(format!("saved {}'s state to {}", t.id, file.display()))
}

/// What the app holds: its memory (`health`, the running app's
/// `health.json`), then each cell's newest stored state and its events.
fn heap(db: &Connection, health: Option<&serde_json::Value>, json: bool) -> Out {
    let mem = match health {
        Some(h) => format!(
            "{} MB resident ({}% of RAM), memory {}",
            h["memory"]["rssBytes"].as_u64().unwrap_or(0) >> 20,
            h["memory"]["machinePct"],
            h["memory"]["level"].as_str().unwrap_or("?")
        ),
        None => "not running: its stored cells only".into(),
    };
    let mut q = db
        .prepare(
            "SELECT c.cell, (SELECT length(state) FROM snap WHERE cell = c.cell ORDER BY seq DESC LIMIT 1),
               (SELECT COUNT(*) FROM log WHERE cell = c.cell), (SELECT COALESCE(SUM(length(args)), 0) FROM log WHERE cell = c.cell)
             FROM (SELECT cell FROM snap UNION SELECT cell FROM log) c ORDER BY c.cell",
        )
        .map_err(|e| e.to_string())?;
    let rows = q
        .query_map([], |r| {
            let (cell, state, n, bytes): (String, Option<i64>, i64, i64) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            if json {
                let row = serde_json::json!({ "cell": cell, "stateBytes": state, "events": n, "argBytes": bytes });
                return Ok(row.to_string());
            }
            let state = state.map_or("-".into(), |b| places::human(b as u64));
            Ok(format!("  {cell:<20} {state:>10} {n:>8} {:>12}", places::human(bytes as u64)))
        })
        .map_err(|e| e.to_string())?;
    let rows: Vec<String> = rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    if json {
        let memory = health.map(|h| h["memory"].clone()).unwrap_or_default();
        return Ok(format!("{{\"memory\":{memory},\"cells\":[{}]}}", rows.join(",")));
    }
    let head = format!("  {:<20} {:>10} {:>8} {:>12}", "cell", "state", "events", "event args");
    Ok(std::iter::once(mem).chain(std::iter::once(head)).chain(rows).collect::<Vec<_>>().join("\n"))
}

fn schedules(db: &Connection, json: bool) -> Out {
    let mut q = db.prepare("SELECT * FROM sched ORDER BY due, cell, key").map_err(|e| e.to_string())?;
    // `cron` is there from store schema 2 on.
    let has_cron = q.column_names().contains(&"cron");
    let rows = q
        .query_map([], |r| {
            let (cell, key, due, every, method): (String, String, i64, Option<i64>, String) =
                (r.get("cell")?, r.get("key")?, r.get("due")?, r.get("every")?, r.get("method")?);
            let cron: Option<String> = if has_cron { r.get("cron")? } else { None };
            if json {
                let row = serde_json::json!({ "due": due, "cell": cell, "key": key, "method": method, "every": every, "cron": cron });
                return Ok(row.to_string());
            }
            let every = every.map_or(String::new(), |ms| format!(" every {ms} ms"));
            let cron = cron.map_or(String::new(), |c| format!(" cron \"{c}\""));
            Ok(format!("{due}  {cell}:{key} → {method}{every}{cron}"))
        })
        .map_err(|e| e.to_string())?;
    let lines: Vec<String> = rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    Ok(match json {
        true => format!("[{}]", lines.join(",")),
        false if lines.is_empty() => "no schedules".into(),
        false => lines.join("\n"),
    })
}

fn runtime(fetch: bool) -> Out {
    use aio_shell::runtime::{VERSION, pinned_sha};
    let dir = aio_shell::runtime::dir();
    let here = match std::env::var_os("AIORT_RUNTIME") {
        Some(_) => aio_shell::runtime::exe(&dir).exists(),
        None => {
            aio_shell::runtime::migrate();
            aio_shell::runtime::usable(std::env::consts::OS, &dir)
        }
    };
    if !fetch && !here {
        return Ok(format!("not installed (would be {}); `ax runtime --fetch` gets it", dir.display()));
    }
    // Fetched with no window (a terminal shows the progress).
    if !here {
        aio_shell::runtime::prefetch(VERSION, pinned_sha(VERSION).as_deref(), &|_| {}).map_err(|e| e.to_string())?;
    }
    aio_shell::runtime::ensure("aio-rs")
        .map(|d| format!("Electron {} at {}", aio_shell::runtime::VERSION, d.display()))
        .map_err(|e| e.to_string())
}

/// The scratch app's record of the ax and pin that made it.
const WARM_STAMP: &str = ".ax-warm";

/// Whether the scratch app at `at` is one this ax made on this pin (`stamp`).
fn scratch_current(at: &Path, stamp: &str) -> bool {
    at.join("Cargo.toml").is_file() && std::fs::read_to_string(at.join(WARM_STAMP)).is_ok_and(|s| s == stamp)
}

/// Whether `ax setup` (the installers' last step) runs `ax setup --warm`'s
/// build before it returns (pure): not for `--check`, else as `ax create` warms.
fn setup_warms(check: bool, terminal: bool, env: Option<&str>) -> bool {
    !check && dev::warms_after_create(terminal, false, env)
}

/// The build folder `ax setup --warm` builds into from `here`: the scratch
/// app's toolchain is the template's pin.
fn warm_target(here: &Path) -> PathBuf {
    let rustup = std::env::var("RUSTUP_TOOLCHAIN").ok();
    dev::shared_target_with(here, rustup.as_deref().or(Some(crate::setup::toolchain())))
}

/// `ax setup --warm`: a scratch app from the template, on this ax's aio-rs
/// (or the checkout `aio`), built as ax dev builds it, in the shared target:
/// every app made after builds only its own crates (a first start of
/// seconds, not the 20 s of aio-rs and its dependencies).
fn setup_warm(aio: Option<&Path>) -> Result<String, String> {
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    let target = warm_target(&here);
    let checkout = aio.map(|a| here.join(a));
    // On a checkout, beside it (an app names its checkout by a relative path).
    let home = match &checkout {
        Some(c) => c.join("target").join("aio-warm"),
        None => target.join("aio").join("warm"),
    };
    let name = "aio-warm";
    let at = home.join(name);
    let mut new = create::New::of(name);
    std::fs::create_dir_all(&home).map_err(|e| format!("{}: {e}", home.display()))?;
    if let Some(c) = &checkout {
        new.aio = create::Aio::checkout(&at, c)?;
    }
    // Made by this ax on this pin, else made again: an older one builds an
    // aio-rs and a template no app made now has.
    let stamp = format!("{} {} {:?}\n", env!("CARGO_PKG_VERSION"), crate::setup::toolchain(), new.aio);
    if !scratch_current(&at, &stamp) {
        _ = std::fs::remove_dir_all(&at);
        create::run(&home, &new, (&target, &places().install), true)?;
        std::fs::write(at.join(WARM_STAMP), &stamp).map_err(|e| format!("{}: {e}", at.display()))?;
    }
    let ax = std::env::current_exe().map_err(|e| e.to_string())?;
    let ok = std::process::Command::new(ax)
        .args(["dev", name, "--warm"])
        .current_dir(&at)
        .status()
        .map_err(|e| e.to_string())?;
    match ok.success() {
        true => Ok(format!("warm: aio-rs and its dependencies built for ax dev in {}", target.display())),
        false => Err(format!("warm: the scratch app's build failed (in {})", at.display())),
    }
}

/// Whether `ax start`'s `args` ask for ax dev's tooling (pure): `--cdp`
/// before a bare `--` (after it, the app's own words).
fn asks_dev(args: &[String]) -> bool {
    crate::cdp::take_flag(args).is_ok_and(|(_, port)| port.is_some())
}

/// The binary `ax start` runs for `t` with `args`: built now in its
/// workspace, else the installed one (its bin link). Built as the release
/// build runs (no operator door, no DevTools port, hardened), but with
/// `--cdp`: asking for DevTools is asking for ax dev's tooling.
fn app_bin(t: &Target, args: &[String]) -> Result<PathBuf, String> {
    match &t.pkg {
        Some(pkg) => dev::build(pkg, Some(&t.plan.logs), asks_dev(args))
            .ok_or_else(|| format!("{}: the build failed (`ax errors {}`)", t.name, t.name)),
        None => {
            let bin = places().bin.join(&t.id);
            if bin.exists() {
                Ok(bin)
            } else {
                Err(format!("{}: not an app of this workspace, and not installed ({})", t.name, bin.display()))
            }
        }
    }
}

/// `ax cost --window`: seconds, or `30s`, `5m`, `1h`; an hour at most (pure).
fn window_ms(v: &str) -> Result<i64, String> {
    let ms = match v.parse::<u64>() {
        Ok(s) => Some(s as i64 * 1000),
        Err(_) if v.ends_with(['s', 'm', 'h']) => logs::since(v, 0).map(|s| -s),
        Err(_) => None,
    };
    match ms {
        Some(ms) if ms > 0 && ms <= aio::cost::KEPT => Ok(ms),
        Some(_) => Err(format!("--window {v}: more than 0, an hour at most (a meter a second is kept that long)")),
        None => Err(format!("--window {v}: seconds, or 30s, 5m, 1h")),
    }
}

/// Rows of a verb over several apps as JSON: the list, or (one failed) an
/// object with the first error and the list.
fn failed_or(rows: Vec<serde_json::Value>) -> String {
    match rows.iter().find_map(|r| r.get("error")) {
        Some(e) => serde_json::json!({ "error": e, "apps": rows }).to_string(),
        None => serde_json::Value::Array(rows).to_string(),
    }
}

/// `ax start --json`: the app as it came up (its `health.json`, its start
/// record), `line` what the start said.
fn started(t: &Target, line: &str, wait: std::time::Duration) -> serde_json::Value {
    let read = |p: PathBuf| std::fs::read(p).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let rec = read(t.plan.logs.join(start::RECORD)).unwrap_or_default();
    let health = if wait.is_zero() { None } else { read(t.plan.data.join("health.json")) };
    let h = health.unwrap_or_default();
    serde_json::json!({
        "app": t.name, "id": t.id, "state": if wait.is_zero() { "spawned" } else { "started" },
        "pid": rec["pid"], "version": h["version"], "startedAt": h["startedAt"],
        "cdp": cdp::recorded(&t.plan), "log": t.plan.logs.join(start::LOG), "text": line,
    })
}

/// The id of the app `name` builds (its `aio.toml`), or `name` itself
/// outside a workspace (an installed app).
fn app_id(name: &str) -> Result<String, String> {
    let Ok(meta) = dev::metadata() else { return Ok(name.into()) };
    match dev::locate(&meta, name) {
        Ok(pkg) => Ok(build::app_of(&pkg)?.0.id),
        Err(_) => Ok(name.into()),
    }
}

/// Where installs go for this user.
fn places() -> installed::Places {
    installed::Places::of(&env, &aio::paths::tool_home(), aio::paths::cache_home())
}

/// A variable, set and not empty.
fn env(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The app's config as its next build bakes it in (`aio::config!()`): its
/// `aio.toml`, checked, every default filled in.
fn config(name: &str) -> Out {
    let pkg = dev::metadata()
        .and_then(|m| dev::locate(&m, name))
        .map_err(|e| format!("{e}\n(an installed app's aio.toml is built into it: run this in its workspace)"))?;
    let file = build::toml_of(&pkg)?;
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    effective(&file, &text)
}

/// `aio.toml` text read from `from`, checked, with every default in.
fn effective(from: &Path, text: &str) -> Out {
    let cfg = aio::config::Config::parse(text).map_err(|e| format!("{}: {e}", from.display()))?;
    serde_json::to_string_pretty(&serde_json::json!({ "from": from, "config": cfg })).map_err(|e| e.to_string())
}

/// `data/health.json`, judged now; `json`: the verdict with the document.
fn health(data: &Path, json: bool) -> Out {
    let file = data.join("health.json");
    let text = std::fs::read(&file).map_err(|_| format!("not running (no {})", file.display()))?;
    let doc: aio::serde_json::Value = aio::serde_json::from_slice(&text).map_err(|e| e.to_string())?;
    let alive = aio::paths::running(data);
    let verdict = aio::health::judge(&doc, now_ms(), alive);
    if !json {
        return verdict;
    }
    let (healthy, text) = match &verdict {
        Ok(t) => (true, t),
        Err(e) => (false, e),
    };
    let v = serde_json::json!({ "healthy": healthy, "error": (!healthy).then_some(text), "text": text, "running": alive, "health": doc });
    if healthy { Ok(v.to_string()) } else { Err(v.to_string()) }
}

/// `ax vitals`: the vitals the running app wrote in its `health.json`.
fn vitals(data: &Path, json: bool) -> Out {
    let file = data.join("health.json");
    let text = std::fs::read(&file).ok().filter(|_| aio::paths::running(data));
    let text = text.ok_or_else(|| format!("not running (no live {})", file.display()))?;
    let doc: serde_json::Value = serde_json::from_slice(&text).map_err(|e| e.to_string())?;
    let v = doc.get("vitals").cloned().ok_or("this app's runtime writes no vitals: update aio-rs")?;
    Ok(if json { v.to_string() } else { vitals_text(&v) })
}

/// The vitals as lines (pure): one per layer, then the likely cause.
fn vitals_text(v: &serde_json::Value) -> String {
    let s = |v: &serde_json::Value| v.as_str().unwrap_or("?").to_string();
    let lp = &v["loop"];
    let mut out = vec![
        format!("loop       {}  {} ms late, {} effects waiting", s(&lp["status"]), lp["lateMs"], lp["queue"]),
        format!("transport  {}  round trip {} ms", s(&v["transport"]["status"]), v["transport"]["measured"]),
        format!("render     {}  {} ms to paint", s(&v["render"]["status"]), v["render"]["measured"]),
    ];
    if let Some(c) = lp["lastCall"]["call"].as_str() {
        out.push(format!("last call  {c} ({} ms)", lp["lastCall"]["ms"]));
    }
    if let Some(h) = v["hint"].as_object() {
        out.push(format!("likely: {} → fix: {}", s(&h["cause"]), s(&h["fix"])));
    }
    out.join("\n")
}

#[cfg(test)]
#[test]
fn vitals_say_each_layer_and_the_likely_cause() {
    let (snap, t) = (aio::vitals::Snapshot::default(), aio::vitals::Vitals::default());
    let v = aio::vitals::doc(&snap, false, &t);
    let text = vitals_text(&v);
    assert!(text.starts_with("loop       healthy  0 ms late, 0 effects waiting\ntransport  healthy"), "{text}");
    assert!(!text.contains("likely"), "nothing to say");
    let frozen = aio::vitals::Snapshot {
        transport: aio::vitals::Layer { status: aio::vitals::Status::Frozen, measured: 5000, since: Some(1) },
        ..snap
    };
    let text = vitals_text(&aio::vitals::doc(&frozen, false, &t));
    assert!(
        text.contains("transport  frozen  round trip 5000 ms") && text.contains("likely: The network link stalled"),
        "{text}"
    );
}

/// The problem reports: a list, or the one whose id starts with `id`.
/// `ax report <app> --new` (aio's `am report`): a problem report filed now.
/// A running app files it itself (its cells, calls and what its bus heard);
/// a stopped one's is written here: its log's newest lines and its last
/// versions, no state (what `#[redact]` withholds is the app's to know).
fn new_report(t: &Target, title: &str, json: bool) -> Out {
    let made = match aio::paths::running(&t.plan.data) {
        true => {
            let ask = serde_json::json!({ "t": "report", "title": title });
            let filed = doors(t).into_iter().find_map(|c| {
                let link = door(t, &c).ok()?;
                aio::rt()
                    .block_on(async { aio::sync::Live::connect(&link, &c, &user(None)).await?.ask(ask.clone()).await })
                    .ok()
            });
            PathBuf::from(
                filed
                    .and_then(|p| p.as_str().map(String::from))
                    .ok_or(format!("{} runs, and filed no report: its doors refused", t.name))?,
            )
        }
        false => {
            let last: serde_json::Value = std::fs::read(t.plan.data.join("health.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            let about = aio::report::About {
                id: t.id.clone(),
                version: last["version"].as_str().unwrap_or("?").into(),
                dir: t.plan.data.join("reports"),
                log: t.plan.logs.join("app.log"),
                channel: String::new(),
                keep: aio::report::KEEP,
                auto: false,
                args: false,
            };
            let at = now_ms();
            let id = format!("{}-ax", chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S-%3fZ"));
            let logs: Vec<String> =
                log_lines(&t.plan.logs, aio::report::LOG_LINES).unwrap_or_default().lines().map(String::from).collect();
            let x = aio::report::Context {
                withheld: vec!["the app wasn't running: no state taken".into()],
                ..Default::default()
            };
            let mut r = aio::report::build(&about, &id, at, "user", title, None, &logs, &x);
            r["app"]["aio"] = last["aio"].clone();
            r["app"]["pid"] = serde_json::Value::Null;
            aio::paths::private_dir(&about.dir).map_err(|e| format!("{}: {e}", about.dir.display()))?;
            let p = about.dir.join(format!("{id}.json"));
            let text = format!("{}\n", serde_json::to_string_pretty(&r).map_err(|e| e.to_string())?);
            aio::paths::write_atomic(&p, text.as_bytes(), true).map_err(|e| format!("{}: {e}", p.display()))?;
            p
        }
    };
    let id = made.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    match json {
        true => Ok(serde_json::json!({ "id": id, "file": made }).to_string()),
        false => Ok(format!("filed report {id}: {} (ax report {} {id})", made.display(), t.name)),
    }
}

fn report(data: &Path, id: Option<&str>, json: bool) -> Out {
    let dir = data.join("reports");
    let all = aio::report::list(&dir);
    match id {
        None if json => {
            let rows = all.iter().map(|(id, v)| serde_json::json!({ "id": id, "title": v["title"] }));
            Ok(serde_json::Value::Array(rows.collect()).to_string())
        }
        None if all.is_empty() => Ok(format!("no problem reports in {}", dir.display())),
        None => Ok(std::iter::once(format!("{} reports in {} (newest first)", all.len(), dir.display()))
            .chain(all.iter().map(|(id, v)| format!("{id}  {}", v["title"].as_str().unwrap_or(""))))
            .collect::<Vec<_>>()
            .join("\n")),
        Some(p) => match all.iter().filter(|(id, _)| id.starts_with(p)).collect::<Vec<_>>().as_slice() {
            [(_, v)] => aio::serde_json::to_string_pretty(v).map_err(|e| e.to_string()),
            [] => Err(format!("no report {p}* in {}", dir.display())),
            many => Err(format!("{} reports start with {p}: give more of the id", many.len())),
        },
    }
}

/// `ax pair <app>` (aio's `am pair`): a fresh pairing code from the running app.
fn pair_code(t: &Target, plain: bool) -> Out {
    let pin = aio::rt().block_on(aio::pairing::mint(&t.plan.data, plain)).map_err(|e| unserved(t, e))?;
    Ok(format!("{pin}\n(valid 3 minutes, once: enter it on the new device)"))
}

/// Why the app serves no other device (aio's `"<id>" is not running —
/// start it`), plainly: not running, or with no server.
fn unserved(t: &Target, e: std::io::Error) -> String {
    let server = t.pkg.as_ref().and_then(|p| build::app_of(p).ok()).map(|(_, c)| c.server.listen.is_some());
    match e.kind() {
        std::io::ErrorKind::NotFound => not_serving(&t.name, aio::paths::running(&t.plan.data), server),
        _ => e.to_string(),
    }
}

/// What [`unserved`] says (pure); `server`: whether its aio.toml gives it
/// one (None: unknown, an installed app).
fn not_serving(name: &str, running: bool, server: Option<bool>) -> String {
    let give = "give it one in its aio.toml ([app] mode = \"client-server\", [server] listen = \"0.0.0.0:8100\")";
    match (running, server) {
        (false, Some(false)) => {
            format!("{name} isn't running, and has no server for other devices: {give}, then `ax start {name}`")
        }
        (false, _) => format!("{name} isn't running: start it with `ax start {name}`"),
        (true, _) => format!("{name} runs, but has no server for other devices: {give}, then `ax restart {name}`"),
    }
}

/// Where a backup of the store at `store` goes unless named: `backups/`
/// in the app's home, beside its data (aio's place: never the cwd, which is
/// the app's checkout, where git would offer to commit it).
fn backup_file(app: &str, store: &Path, ms: i64) -> PathBuf {
    let home = store.parent().and_then(Path::parent).unwrap_or(Path::new("."));
    home.join("backups").join(format!("{app}-{}.aio-backup", aio::log::stamp(ms).replace([':', '.'], "-")))
}

fn backup(t: &Target, file: Option<&str>, json: bool) -> Out {
    let (app, store) = (&t.id, t.store());
    let file = match file {
        Some(f) => PathBuf::from(f),
        None => {
            let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
            let f = backup_file(app, &store, ms as i64);
            if store.exists() {
                aio::paths::private_dir(f.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
            }
            f
        }
    };
    aio::backup::backup_all(app, &store, &file).map_err(|e| e.to_string())?;
    if json {
        let bytes = std::fs::metadata(&file).map_or(0, |m| m.len());
        return Ok(serde_json::json!({ "app": app, "file": file, "bytes": bytes }).to_string());
    }
    Ok(format!("backed up {app} to {}\n  put it back with: ax restore {app} {}", file.display(), file.display()))
}

fn restore(t: &Target, file: &str) -> Out {
    // Holding the app's lock proves it isn't running.
    let app = &t.id;
    let dirs = aio::paths::AppDirs::planned(app, t.plan.clone()).map_err(|e| e.to_string())?;
    // `ax status` says maintenance meanwhile.
    let aside =
        status::held(&dirs.data, app, "restore", || aio::backup::restore(app, Path::new(file), &dirs.store, &dirs))
            .and_then(|r| r)
            .map_err(|e| e.to_string())?;
    Ok(match aside {
        Some(a) => format!("restored {app} from {file}; the data it replaced is kept as {}", a.display()),
        None => format!("restored {app} from {file}"),
    })
}

fn pair(t: &Target, addr: &str, code: &str) -> Out {
    let app = &t.id;
    let tls = aio::rt().block_on(aio::connect::pair(&t.plan.data, app, addr, code)).map_err(|e| e.to_string())?;
    Ok(format!(
        "paired {app} with {addr}{}",
        if tls { " (TLS, root pinned)" } else { " (no TLS: use it only on a network you trust)" }
    ))
}

/// `ax profile`: the profile, printed or written 0600 (it holds the key).
/// A browser link that carries the app's key (`ax profile --link`, aio's
/// `share:` link; pure): the key goes once as `?token=`, which sets a cookie.
fn share_link(p: &aio::serde_json::Value) -> String {
    let host = p["host"].as_str().unwrap_or("127.0.0.1");
    let host = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
    let key: String = p["key"]
        .as_str()
        .unwrap_or_default()
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    let scheme = if p["tls"].as_bool() == Some(true) { "https" } else { "http" };
    format!("{scheme}://{host}:{}/?token={key}", p["port"])
}

fn profile(t: &Target, out: Option<&str>, link: bool) -> Out {
    let (p, local) = aio::pairing::local_profile(&t.plan.data, &t.id).map_err(|e| unserved(t, e))?;
    if local {
        eprintln!(
            "[aio] {} listens on this machine only, so this profile connects from here only: \
             listen on 0.0.0.0 to hand it to another device",
            t.name
        );
    }
    if link {
        return Ok(share_link(&p));
    }
    let text = aio::serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?;
    match out {
        None => Ok(text),
        Some(f) => {
            aio::paths::write_atomic(Path::new(f), text.as_bytes(), true).map_err(|e| format!("{f}: {e}"))?;
            Ok(format!("wrote {f} (0600: it holds the app's key)"))
        }
    }
}

/// `text` into file `f` (`-`: the text itself, for stdout), saying `what` it holds.
fn write_or_say(f: &str, text: String, what: &str) -> Out {
    if f == "-" {
        return Ok(text);
    }
    std::fs::write(f, text).map_err(|e| format!("{f}: {e}"))?;
    Ok(format!("wrote {f}: {what}"))
}

/// Run a verb as `verbs::read` gave it: the verb, its flags, `--`, its words.
fn run(args: &[String]) -> Out {
    let cut = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (flags, pos) = (&args[1..cut], args.get(cut + 1..).unwrap_or_default());
    let has = |f: &str| flags.iter().any(|a| a == f);
    let json = has("--json");
    if args[0] == "build" && has("--quick") {
        if has("--sizes") {
            return Err("--quick is a build to try: --sizes measures the one you ship (ax build <app> --sizes)".into());
        }
        freeze::quick();
        eprintln!("ax build: --quick: no LTO, 16 codegen units (a bigger, slower binary): to try, never to ship");
    }
    let value = |f: &str| flags.iter().position(|a| a == f).and_then(|i| flags.get(i + 1)).map(String::as_str);
    let last: Option<u32> = match value("--last") {
        Some(n) => Some(n.parse().map_err(|_| "--last needs a number")?),
        None => None,
    };
    let p: Vec<&str> = pos.iter().map(String::as_str).collect();
    let ask = aio::paths::Ask {
        home: value("--home").map(String::from),
        profile: value("--profile").map(String::from),
        instance: value("--instance").map(String::from),
        ..Default::default()
    };
    let t = |app: &str| target::of(app, &ask);
    // `--wait <s>`: how long a stopping app is given before SIGTERM.
    let wait = || match value("--wait") {
        Some(s) => {
            s.parse().map(std::time::Duration::from_secs).map_err(|_| format!("--wait {s}: a number of seconds"))
        }
        None => Ok(std::time::Duration::from_secs(15)),
    };
    // `--window <n>`: which of the app's windows (0, the first).
    let window = || match value("--window") {
        Some(n) => n.parse::<usize>().map_err(|_| format!("--window {n}: 0, 1, 2…")),
        None => Ok(0),
    };
    // `--wait-client <ms>`: how long surface and trigger wait for the window.
    let wait_client = || match value("--wait-client") {
        Some(ms) => {
            ms.parse().map(std::time::Duration::from_millis).map_err(|_| format!("--wait-client {ms}: milliseconds"))
        }
        None => Ok(std::time::Duration::ZERO),
    };
    // The window's DevTools protocol: `--cdp`'s port, else the app's own door.
    let reach = |t: &Target| cdp::reach(value("--cdp"), cdp::recorded(&t.plan), cdp::door(&t.id, &t.plan));
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    if let Some(v) = build_version(value("--app-version"), var("AIORT_BUILD_VERSION"), var("AIO_BUILD_VERSION")) {
        version::ask(&v)?;
    }
    if let Some(d) = value("--out") {
        build::ask_out(d)?;
    }
    match (args[0].as_str(), p.as_slice()) {
        ("dev", [name, "--android", rest @ ..]) => {
            let (mut device, mut native, mut it) = (None, false, rest.iter());
            while let Some(a) = it.next() {
                match *a {
                    "--device" => device = Some(*it.next().ok_or("--device needs a serial")?),
                    "--native" => native = true,
                    _ => return Err("ax dev <app> --android [--device serial] [--native]".into()),
                }
            }
            dev::android(name, device, native)
        }
        ("dev", [name, rest @ ..]) if rest.contains(&"--web") => web::dev(name, web::Serve::parse(rest)?),
        ("dev", [name, ..]) => dev::run(name, &pos[1..]),
        ("check", [name]) => check::run(name, has("--release")),
        ("test", [name]) if has("--android") => install::lane(name, value("--device")),
        ("test", [name]) if has("--mutants") => testrun::mutants(name),
        ("test", [name, filter @ ..]) if filter.len() <= 1 => {
            testrun::run(name, filter.first().copied(), value("--video"), (value("--pace"), value("--video-scheme")))
        }
        ("preview", [app, component, props @ ..]) if props.len() <= 1 => {
            let (lib, dir) = app_lib(app)?;
            preview::run(&lib, &dir, component, props.first().copied(), has("--html"))
        }
        ("create", [name]) => {
            let here = std::env::current_dir().map_err(|e| e.to_string())?;
            let aio = match (value("--aio"), value("--aio-version")) {
                (Some(_), Some(_)) => return Err("--aio (a checkout) or --aio-version (a release), not both".into()),
                (Some(d), None) => create::Aio::checkout(&here.join(name), Path::new(d))?,
                (None, Some(v)) => create::Aio::version(v, create::tags)?,
                (None, None) => create::Aio::this(),
            };
            let css = match value("--css") {
                None => false,
                Some("tailwind") => true,
                Some(c) => return Err(format!("--css {c}: tailwind (any other tool: [build] css in aio.toml)")),
            };
            let maintainer = create::git_maintainer(&here);
            let new = create::New {
                template: value("--template").unwrap_or("app"),
                id: value("--id").unwrap_or(name),
                maintainer: maintainer.as_deref().unwrap_or(create::NO_MAINTAINER),
                aio,
                targets: value("--target").map_or(vec![], |t| t.split(',').map(str::trim).collect()),
                tailwind: css,
                ..create::New::of(name)
            };
            // The folder the new app's builds go to: its toolchain is the template's pin.
            let rustup = std::env::var("RUSTUP_TOOLCHAIN").ok();
            let pin = rustup.as_deref().or(Some(crate::setup::toolchain()));
            let (target, installed) = (dev::shared_target_with(&here, pin), places().install);
            let mut made = create::run(&here, &new, (&target, &installed), has("--force"))?;
            if let (Some(dir), create::Aio::Tag(t)) = (create::this_checkout(), &new.aio) {
                made.notes.push(format!(
                    "note: this ax is built from the checkout {}: its templates may be newer than {t}; \
                     --aio <that checkout> builds the app on it",
                    dir.display()
                ));
            }
            made.notes.extend(create::lock(&made.at));
            made.notes.push(create::git_init(&made.at, maintainer.is_some()));
            // The first `ax dev` is a launch, not a 20 s build: built now, in the background.
            let terminal = std::io::IsTerminal::is_terminal(&std::io::stderr()) && !json;
            if dev::warms_after_create(terminal, has("--no-warm"), std::env::var("AIORT_NO_WARM").ok().as_deref()) {
                let log = target.join("aio").join(format!("warm-{}.log", made.name));
                made.notes.push(match dev::warm_in_background(&made.at, &made.name, &log) {
                    Ok(()) => format!(
                        "building {} for ax dev in the background ({}), so its first start opens at once (--no-warm: not)",
                        made.name,
                        log.display()
                    ),
                    Err(e) => format!("not built ahead: {e}"),
                });
            }
            Ok(made.said(json))
        }
        ("add", [app, "page", name]) => add::page(&pin::root_of(Some(app))?, name, json),
        ("add", [app, kind, name]) => add::run(&pin::root_of(Some(app))?, add::Kind::of(kind)?, name, json),
        ("pin", [w]) if pin::is_target(w) => pin::run(None, Some(w), (has("--major"), has("--force")), json),
        ("pin", [app, rest @ ..]) if rest.len() <= 1 => {
            pin::run(Some(app), rest.first().copied(), (has("--major"), has("--force")), json)
        }
        ("upgrade", rest) if rest.len() <= 1 => upgrade::upgrade(rest.first().copied(), value("--aio").map(Path::new)),
        ("uninstall", []) => {
            upgrade::uninstall(has("--tools"), &aio::paths::tool_home(), &aio::paths::cache_home(), &upgrade::bin())
        }
        ("keygen", []) if has("--aio") => release::keygen_aio(&release::keys_dir(), has("--rotate")),
        ("keygen", [name]) => release::keygen(&release::keys_dir(), &app_id(name)?, has("--rotate")),
        ("release", []) if has("--aio") => {
            // --target: another one this machine builds (a Mac's other CPU: x86_64-apple-darwin).
            let targets: Vec<&str> =
                has("--windows").then_some("x86_64-pc-windows-gnu").into_iter().chain(value("--target")).collect();
            release::aio(&targets, value("--dir").map(Path::new), value("--url").unwrap_or(""))
        }
        ("release", [name]) if has("--workflow") => {
            release::write_workflow(name, value("--channel"), (has("--pages"), has("--web")))
        }
        ("release", [name]) => {
            let list = |f| value(f).map(|v| v.split(',').filter(|v| !v.is_empty()).collect()).unwrap_or_default();
            let mut platforms: Vec<&str> = list("--platforms");
            platforms.extend(has("--windows").then_some("windows"));
            let ask = release::Ask {
                channel: value("--channel"),
                notes: value("--notes").unwrap_or(""),
                platforms: platform::resolve(&platforms, platform::host()?)?,
                patch_from: list("--patch-from"),
                min_from: value("--min-from"),
                commit: has("--commit"),
                dir: value("--dir").map(Path::new),
                data: value("--data").map(Path::new),
                no_build: has("--no-build"),
                allow_dirty: has("--allow-dirty"),
                appimage: has("--appimage"),
                unsigned: has("--unsigned"),
                key: value("--key").map(Path::new),
            };
            match has("--android") {
                true => release::run_android(name, &ask),
                _ => release::run(name, &ask),
            }
        }
        ("update", [name]) => match value("--from-git") {
            Some(r) => release::from_git(name, r, &places().apps),
            None => release::status(&app_id(name)?),
        },
        ("installed", []) => Ok(installed::list(&places(), json)),
        ("remove", [name]) => installed::remove(&places(), &app_id(name)?, has("--data")),
        ("install", [app]) if !has("--android") => {
            let (built, out) = build::make(app, None, None)?;
            let running = aio::paths::running(&places().apps.join(&built.app.id).join("data"));
            let note =
                if running { format!("\n{app} runs the old version until `ax restart {app}`") } else { String::new() };
            #[cfg(unix)]
            if built.platform.os == "linux" && has("--appimage") {
                let image = package::appimage(&built)?;
                return installed::from_appimage(&places(), &built.app, &image).map(|s| s + &note);
            }
            #[cfg(unix)]
            if built.platform.os == "linux" {
                return installed::from_bundle(&out, &[]).map(|s| s + &note);
            }
            Ok(format!("{}\n{}{note}", out.display(), bundle::how_to_install(&built.app, built.platform.os, &out)))
        }
        ("install", [app]) => install::android(
            app,
            &install::Opts {
                build: has("--build"),
                emulator: has("--emulator"),
                apk: value("--apk"),
                device: value("--device"),
                launch: !has("--no-launch"),
            },
        ),
        // A machine named for the OS (AIO_LAB_*): over ssh; else (or --boot) a container here.
        ("lab", [app, os]) => {
            let machine = lab::machine(os, &|k| std::env::var(k).ok().filter(|v| !v.is_empty())).is_ok();
            let boxed = [
                "--boot",
                "--status",
                "--reset",
                "--ram",
                "--cpus",
                "--disk",
                "--os-version",
                "--apk",
                "--port",
                "--dist",
                "--tunnel",
            ];
            if machine && !boxed.iter().any(|f| has(f)) {
                return lab::run(app, os, has("--stop"));
            }
            let port = match value("--port") {
                Some(p) => Some(p.parse().map_err(|_| format!("--port {p}: a port"))?),
                None => None,
            };
            let ask = boxlab::Ask {
                ram: value("--ram"),
                cpus: value("--cpus"),
                disk: value("--disk"),
                version: value("--os-version"),
                port,
                apk: value("--apk"),
                tunnel: has("--tunnel"),
            };
            let what = ["status", "stop", "reset"].into_iter().find(|w| has(&format!("--{w}"))).unwrap_or("boot");
            let meta = dev::metadata()?;
            let dist = match value("--dist") {
                // Docker mounts an absolute path.
                Some(d) => std::path::absolute(d).map_err(|e| format!("--dist {d}: {e}"))?,
                None => build::dist_of(&meta, app)?,
            };
            boxlab::run(os, what, &ask, &dist)
        }
        ("smoke", [artifact]) => {
            let file = Path::new(artifact);
            let id =
                smoke::id_of(file).ok_or_else(|| format!("{artifact}: not <id>-<version>-… as ax build names it"))?;
            match smoke::is_windows_zip(file) {
                true => smoke::windows(file, id, smoke::Wait::Window),
                false => smoke::run(file, id, smoke::Wait::Window),
            }
        }
        ("build", [app]) if has("--list") => build::list(app),
        ("build", [app]) if has("--sizes") => build::sizes_of(app),
        ("build", [_]) if has("--dev") && !has("--web") => {
            Err("--dev is a web build's: ax build <app> --web --dev".into())
        }
        ("build", [app]) if has("--debug") => match has("--android") {
            true => build::android(app, android::Ask { debug: true, ..android::Ask::RELEASE }).map(|(said, _)| said),
            false => Err("--debug is an Android build's: ax build <app> --android --debug".into()),
        },
        ("build", [app]) if has("--web") => web::build(app, has("--dev")).map(|(said, _)| said),
        ("build", [app]) if has("--ios") => ios::build(app).map(|(said, _)| said),
        ("build", [app]) => {
            // The kind flags name targets too; --deb and --appimage come with the bundle.
            let mut select: Vec<&str> = value("--targets").into_iter().collect();
            select.extend(["--android", "--windows"].iter().filter(|f| has(f)).map(|f| &f[2..]));
            if has("--deb") || has("--appimage") {
                select.push("desktop");
            }
            select.extend(["--deb", "--appimage"].iter().filter(|f| has(f)).map(|f| &f[2..]));
            let select = select.join(",");
            if has("--force") {
                eprintln!("ax build: {}", build::force(&dev::metadata()?)?);
            }
            let since = std::time::SystemTime::now();
            let smoke = match (has("--smoke-strict"), has("--smoke")) {
                (true, _) => smoke::Smoke::Strict,
                (false, true) => smoke::Smoke::On,
                _ => smoke::Smoke::Off,
            };
            let built = build::run(app, Some(select.as_str()).filter(|s| !s.is_empty()), value("--platforms"), smoke);
            match (json, build::manifest_of(app, since)) {
                (true, Some(m)) => {
                    let v = |text: &str, ok: bool| {
                        serde_json::json!({ "error": (!ok).then_some(text), "text": text, "manifest": m }).to_string()
                    };
                    built.map(|t| v(&t, true)).map_err(|e| v(&e, false))
                }
                _ => built,
            }
        }
        ("agent", section) => {
            agent::brief(section.first().copied(), agent::Size::of(has("--min"), has("--max"))?, has("--list"), json)
        }
        ("apps", []) => Ok(places::apps(&aio::paths::tool_home(), json)),
        ("ui", []) => ui::open(has("--window"), has("--browser")),
        ("open", [app]) => t(app).and_then(|t| ui::open_app(app, &t.plan.data, has("--print"))),
        ("data", [app]) => t(app).map(|t| places::data(&t.id, &t.plan, json)),
        ("config", [app]) => config(app),
        ("discover", []) => {
            let wait = match value("--timeout") {
                Some(ms) => {
                    std::time::Duration::from_millis(ms.parse().map_err(|_| format!("--timeout {ms}: milliseconds"))?)
                }
                None => aio::discovery::WAIT,
            };
            let found = aio::rt().block_on(aio::discovery::probe_lan(aio::discovery::port(), wait));
            found.map(|f| places::discovered(&f, json)).map_err(|e| e.to_string())
        }
        ("feedback", name) => places::feedback(&places::feedback_dir(&env), name.first().copied(), has("--create")),
        ("events" | "timeline", [app, cell @ ..]) => {
            let (cell, n) = (cell.first().copied(), last.unwrap_or(20));
            let t = t(app)?;
            // `--from`: another store (a backup, a copy) read as the journal.
            let from = value("--from").map(PathBuf::from);
            let file = from.clone().unwrap_or_else(|| t.store());
            let db = open(&file)?;
            if args[0] == "events" && !has("--diff") {
                let out = events(&db, cell, n, json, &|_| vec![])?;
                if !json || from.is_some() || !aio::paths::running(&t.plan.data) {
                    return Ok(out);
                }
                // The running app's own readings: each event's time, its refused calls.
                let mut rows: Vec<serde_json::Value> = serde_json::from_str(&out).map_err(|e| e.to_string())?;
                let mut cells: Vec<String> = rows.iter().filter_map(|r| r["cell"].as_str().map(String::from)).collect();
                if has("--refused") {
                    cells.extend(cell.map(String::from).map_or_else(|| metrics::cells(&db), |c| Ok(vec![c]))?);
                }
                cells.sort();
                cells.dedup();
                let read: Vec<(String, serde_json::Value)> = cells
                    .into_iter()
                    .filter_map(|c| {
                        let seqs: Vec<&serde_json::Value> =
                            rows.iter().filter(|r| r["cell"] == c.as_str()).map(|r| &r["seq"]).collect();
                        let ask = serde_json::json!({ "t": "took", "seqs": seqs });
                        let (v, _) = travel::ask(&door(&t, &c).ok()?, &c, &user(None), ask).ok()?;
                        Some((c, v))
                    })
                    .collect();
                timed(&mut rows, &read, has("--refused"));
                return Ok(serde_json::Value::from(rows).to_string());
            }
            // Each event shown with the state before and after it, of its cell.
            let mut q = db
                .prepare("SELECT seq, cell FROM log WHERE ?1 IS NULL OR cell = ?1 ORDER BY seq DESC LIMIT ?2")
                .map_err(|e| e.to_string())?;
            let shown: Vec<(u64, String)> = q
                .query_map(params![cell, n], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))
                .and_then(Iterator::collect)
                .map_err(|e| e.to_string())?;
            let mut ask: std::collections::BTreeMap<String, Vec<u64>> = Default::default();
            for (seq, c) in &shown {
                ask.entry(c.clone()).or_default().push(*seq);
            }
            let at = states_of(&t, &file, from.is_some(), &ask)?;
            let of: std::collections::HashMap<u64, &String> = shown.iter().map(|(s, c)| (*s, c)).collect();
            let none = serde_json::Value::Null;
            let diff = |seq: i64| {
                let seq = seq as u64;
                let Some(states) = of.get(&seq).and_then(|c| at.get(*c)) else { return vec![] };
                travel::diff(states.get(&seq.saturating_sub(1)).unwrap_or(&none), states.get(&seq).unwrap_or(&none), "")
            };
            events(&db, cell, n, json, &diff)
        }
        ("errors", [app]) => {
            let logs = t(app)?.plan.logs;
            let build = std::fs::read_to_string(logs.join(dev::BUILD_LOG)).ok();
            let text = log_lines(&logs, 100_000).unwrap_or_default();
            let (since, reload) = match value("--since") {
                None => (None, None),
                Some("reload") => {
                    let patched =
                        std::fs::read_to_string(logs.join(dev::PATCHED)).ok().and_then(|s| s.trim().parse().ok());
                    (None, Some(patched))
                }
                Some(v) => (
                    Some(logs::since(v, now_ms()).ok_or(format!(
                        "--since {v}: a duration (30s, 15m, 2h, 1d), a time (2026-10-28T10:00) or reload"
                    ))?),
                    None,
                ),
            };
            let lines: Vec<&str> = text.lines().collect();
            Ok(errors(build.as_deref(), newer(&lines, since, reload), last.unwrap_or(20) as usize, json))
        }
        ("logs", [app, word @ ..]) => {
            let f: Vec<&str> = flags.iter().map(String::as_str).collect();
            let mut f = logs::filter(&f, now_ms())?;
            f.text = word.first().map(|w| w.to_string());
            let (dir, n) = (t(app)?.plan.logs, last.unwrap_or(50) as usize);
            if has("--follow") { logs::watch(&dir, n, &f) } else { logs::tail(&dir, n, &f, json) }
        }
        ("health", [app]) => health(&t(app)?.plan.data, json),
        ("vitals", [app]) => vitals(&t(app)?.plan.data, json),
        ("stop", []) => stop::all(if has("--no-wait") { None } else { Some(wait()?) }, json),
        ("stop", apps) => {
            let wait = if has("--no-wait") { None } else { Some(wait()?) };
            let all: Vec<Result<stop::Stopped, String>> =
                apps.iter().map(|app| stop::stopped(app, &t(app)?, false, wait)).collect();
            let ok = all.iter().all(Result::is_ok);
            let text = match json {
                true => {
                    let one = |r: &Result<stop::Stopped, String>| match r {
                        Ok(s) => s.json(),
                        Err(e) => serde_json::json!({ "error": e }),
                    };
                    match all.as_slice() {
                        [r] if ok => one(r).to_string(),
                        _ => failed_or(all.iter().map(one).collect()),
                    }
                }
                false => all
                    .iter()
                    .map(|r| r.as_ref().map_or_else(|e| e.clone(), stop::Stopped::said))
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            if ok { Ok(text) } else { Err(text) }
        }
        ("kill", []) if has("--stale") => stop::kill_stale(&stop::stale(), json),
        ("kill", [app]) => {
            let s = stop::stopped(app, &t(app)?, true, Some(std::time::Duration::ZERO))?;
            Ok(if json { s.json().to_string() } else { s.said() })
        }
        ("start" | "restart", []) => {
            let wait = match value("--wait") {
                _ if has("--no-wait") => std::time::Duration::ZERO,
                Some(_) => wait()?,
                None => start::WAIT,
            };
            let typed: Vec<String> = has("--cdp").then(|| "--cdp".to_string()).into_iter().collect();
            let one = |n: &str| {
                let t = t(n)?;
                let line = match args[0].as_str() {
                    "restart" => start::restart(n, &t, &|a| app_bin(&t, a), &typed, wait),
                    _ => start::run(&t, &app_bin(&t, &typed)?, &typed, value("--display"), wait),
                }?;
                Ok::<_, String>(if json { started(&t, &line, wait).to_string() } else { line })
            };
            let names = stop::workspace_apps()?;
            match json {
                true => {
                    let rows: Vec<serde_json::Value> = names
                        .iter()
                        .map(|n| match one(n) {
                            Ok(v) => serde_json::from_str(&v).unwrap_or_default(),
                            Err(e) if e.contains("is already running") => {
                                serde_json::json!({ "app": n, "state": "started", "already": true })
                            }
                            Err(e) => serde_json::json!({ "app": n, "error": e }),
                        })
                        .collect();
                    let text = failed_or(rows);
                    if text.starts_with('[') { Ok(text) } else { Err(text) }
                }
                false => start::each(&names, &one),
            }
        }
        ("watch", [app, dirs @ ..]) => {
            let t = t(app)?;
            let root = pin::root_of(Some(app))?;
            let dirs: Vec<PathBuf> = match dirs {
                [] => vec![root.join("src")],
                d => d.iter().map(|d| root.join(d)).collect(),
            };
            if let Some(d) = dirs.iter().find(|d| !d.is_dir()) {
                return Err(format!("ax watch: {} is no folder of {app}: name one to watch (src)", d.display()));
            }
            let (_w, saves) = watch::Watch::new(&dirs)?;
            let typed = start::home_args(&ask);
            let wait = wait()?;
            let said = |r: Out| match r {
                Ok(line) => println!("{line}"),
                Err(e) => eprintln!("ax watch: {e} (the last good build runs on; the next save tries again)"),
            };
            eprintln!(
                "ax watch: {} for changes: the app restarts, built anew, on each save (Ctrl-C ends it)",
                watch::describe(&dirs, &root)
            );
            start::watch(
                &saves,
                aio::paths::running(&t.plan.data),
                &|| start::run(&t, &app_bin(&t, &typed)?, &typed, value("--display"), start::WAIT),
                &|| start::restart(app, &t, &|a| app_bin(&t, a), &typed, wait),
                &said,
                &root,
            );
            Err("ax watch: its watcher stopped".into())
        }
        ("run", [link, app @ ..]) if app.len() <= 1 => {
            fetched::run(link, app.first().copied(), value("--ref"), has("--yes"), has("--dev"))
        }
        ("start" | "restart", [app, words @ ..]) => {
            let t = t(app)?;
            let wait = match value("--wait") {
                _ if has("--no-wait") => std::time::Duration::ZERO,
                Some(_) => wait()?,
                None => start::WAIT,
            };
            let mut typed = start::home_args(&ask);
            typed.extend(words.iter().map(|w| w.to_string()));
            typed.extend(has("--cdp").then(|| "--cdp".to_string()));
            let line = match args[0].as_str() {
                "restart" => start::restart(app, &t, &|a| app_bin(&t, a), &typed, wait)?,
                _ => {
                    if has("--fresh") {
                        dev::throw_away(Some(&t.plan))?;
                    }
                    start::run(&t, &app_bin(&t, &typed)?, &typed, value("--display"), wait)?
                }
            };
            Ok(if json { started(&t, &line, wait).to_string() } else { line })
        }
        ("metrics", [app]) => {
            let t = t(app)?;
            let db = open(&t.store())?;
            let health: Option<serde_json::Value> = aio::paths::running(&t.plan.data)
                .then(|| std::fs::read(t.plan.data.join("health.json")).ok())
                .flatten()
                .and_then(|b| serde_json::from_slice(&b).ok());
            let cells = metrics::uses(&db, &|cell| door(&t, cell).ok(), &user(value("--as")))?;
            metrics::show(health.as_ref(), &cells, metrics::schedules(&db)?, json)
        }
        ("report", [app]) if has("--new") => {
            new_report(&t(app)?, value("--title").unwrap_or("filed by ax report"), json)
        }
        ("report", [app]) => report(&t(app)?.plan.data, None, json),
        ("report", [app, id]) => report(&t(app)?.plan.data, Some(id), json),
        ("state", [app, cell, path @ ..]) if has("--watch") => {
            live::watch(&door(&t(app)?, cell)?, cell, path.first().copied(), &user(value("--as")))
        }
        // Under --json, the state alone (the path "" is all of it).
        ("state", [app, cell]) => state(&t(app)?, cell, json.then_some(""), value("--as")),
        ("state", [app, cell, path]) => state(&t(app)?, cell, Some(path), value("--as")),
        ("call", [app, words @ ..]) if value("--fn").is_some() => {
            let t = t(app)?;
            let args = call_body(words, value("--args"), value("--body"))?;
            let name = value("--fn").unwrap_or_default();
            // Server functions are served on the doors of the cells whose launch gave them: each is tried.
            let who = user(value("--as"));
            let mut last = Err(format!("{} has no cell to reach it by", t.name));
            for cell in doors(&t) {
                let Ok(link) = door(&t, &cell) else { continue };
                let wait = call_wait(value("--wait"))?;
                last = live::call_within(
                    &link,
                    &cell,
                    live::Called::Fn(name),
                    args.clone(),
                    (&who, has("--as-server")),
                    wait,
                );
                if !last.as_ref().is_err_and(|e| e.contains("is not served by this app")) {
                    break;
                }
            }
            last
        }
        ("call", [app, cell, method, words @ ..]) => {
            let args = call_body(words, value("--args"), value("--body"))?;
            let who = user(value("--as"));
            let link = door(&t(app)?, cell)?;
            live::call_within(
                &link,
                cell,
                live::Called::Method(method),
                args,
                (&who, has("--as-server")),
                call_wait(value("--wait"))?,
            )
        }
        ("expect", [app, cell, path, op, want @ ..]) => {
            let wait = match value("--wait") {
                Some(s) => std::time::Duration::from_secs_f64(s.parse().map_err(|_| "--wait needs seconds")?),
                None => std::time::Duration::ZERO,
            };
            live::expect_usage(op, want.first().copied())?;
            let passed = live::expect(
                &door(&t(app)?, cell)?,
                cell,
                (path, op, want.first().copied()),
                wait,
                &user(value("--as")),
            );
            match json {
                true => {
                    let v = |pass: bool, text: &str| {
                        serde_json::json!({
                            "pass": pass, "error": (!pass).then_some(text), "cell": cell, "path": path, "op": op,
                            "value": want.first(), "text": text,
                        })
                        .to_string()
                    };
                    passed.map(|t| v(true, &t)).map_err(|e| v(false, &e))
                }
                false => passed,
            }
        }
        ("clients", [app]) => {
            let at = reach(&t(app)?)?;
            aio::rt().block_on(cdp::windows(&at))
        }
        ("clients", [app, cell]) => live::clients(&door(&t(app)?, cell)?, cell, &user(None), json),
        ("shot", [app]) if value("--video").is_some() => {
            let t = t(app)?;
            let still = ["--check", "--update", "--threshold", "--max-diff", "--selector", "--full", "--size"];
            if let Some(f) = still.iter().find(|f| has(f)) {
                return Err(format!("{f} shapes one picture, not a video: leave it out with --video"));
            }
            let until = match value("--duration") {
                Some(s) => Some(
                    s.parse()
                        .ok()
                        .filter(|s: &f64| *s > 0.0 && s.is_finite())
                        .map(std::time::Duration::from_secs_f64)
                        .ok_or(format!("--duration {s}: seconds, more than 0"))?,
                ),
                None => None,
            };
            let at = reach(&t)?;
            let journal = t.plan.logs.join(video::JOURNAL);
            let journal = has("--markers").then_some(journal.as_path());
            video::run(Path::new(value("--video").unwrap_or_default()), &at, until, journal, json)
        }
        ("shot", [app, file @ ..]) if file.len() <= 1 => {
            if let Some(f) = ["--duration", "--markers"].iter().find(|f| has(f)) {
                return Err(format!("{f} is a --video's"));
            }
            let t = t(app)?;
            let out = match file.first() {
                Some(f) => PathBuf::from(f),
                None => PathBuf::from(format!("{}-{}.png", t.id, chrono::Utc::now().format("%Y%m%d-%H%M%S"))),
            };
            let base = match (value("--check"), value("--update")) {
                (Some(_), Some(_)) => return Err("--check and --update are opposites: pick one".into()),
                (Some(b), None) => shot::Baseline::Check(Path::new(b)),
                (None, Some(b)) => shot::Baseline::Update(Path::new(b)),
                (None, None) => shot::Baseline::None,
            };
            let tol = shot::tolerance(value("--threshold"), value("--max-diff"))?;
            let size = value("--size").map(shot::size).transpose()?;
            let at = reach(&t)?;
            shot::run(&out, (&at, window()?), (value("--selector"), has("--full"), size), base, tol, json)
        }
        ("eval", [app, expr]) => {
            let at = reach(&t(app)?)?;
            eval::run(&at, window()?, expr)
        }
        ("surface", [app]) if has("--headless") => {
            let (lib, dir) = app_lib(app)?;
            let t = t(app)?;
            headless::run(&lib, &dir, value("--root").unwrap_or("App"), &t.id, &t.plan.data, &user(None))
        }
        ("surface", [app, path @ ..]) if path.len() <= 1 => {
            let at = reach(&t(app)?)?;
            aio::rt().block_on(async {
                let mut s = cdp::window_within(&at, window()?, wait_client()?).await?;
                match has("--names") {
                    true => surface::names(&mut s).await.map(|n| n.join("\n")),
                    false => {
                        let depth = match value("--depth") {
                            Some(d) => Some(d.parse().map_err(|_| format!("--depth {d}: 0, 1, 2…"))?),
                            None => None,
                        };
                        let r = surface::Read {
                            path: path.first().copied(),
                            rects: has("--rects"),
                            component: value("--component"),
                            depth,
                            full: has("--full"),
                        };
                        surface::read(&mut s, &r).await
                    }
                }
            })
        }
        ("testgen", [app]) => {
            // Beside the app's own tests, unless --out names a file.
            let out = match value("--out") {
                Some(f) => PathBuf::from(f),
                None => app_lib(app).map_or_else(|_| PathBuf::from(testgen::OUT), |(_, dir)| dir.join(testgen::OUT)),
            };
            let at = reach(&t(app)?)?;
            aio::rt().block_on(async {
                let mut s = cdp::window_within(&at, window()?, wait_client()?).await?;
                testgen::run(&mut s, app, &out).await
            })
        }
        ("where", [app]) => runs::run(app, None, json),
        ("where", [app, item]) => runs::run(app, Some(item), json),
        ("tsgen", [app]) => {
            let (_, dir) = app_lib(app)?;
            let out = value("--out").map_or_else(|| dir.join(tsgen::OUT).display().to_string(), String::from);
            let ts = tsgen::render(app, &tsgen::sources(&dir)?)?;
            write_or_say(&out, ts, "its shared cells' state and #[client] methods, typed")
        }
        ("control", [app]) => {
            let port = match value("--port") {
                Some(p) => p.parse().map_err(|_| format!("--port {p}: a port"))?,
                None => 0,
            };
            t(app)?;
            let run = |a: &[String]| {
                let (code, out) = outcome(verbs::read(a, &|| Ok(app.to_string())));
                (code, as_json(code, &out))
            };
            control::serve(app, port, &run)
        }
        ("components", [app]) => {
            let at = reach(&t(app)?)?;
            aio::rt().block_on(async {
                let mut s = cdp::window_within(&at, window()?, wait_client()?).await?;
                components::run(&mut s, json, has("--renders")).await
            })
        }
        ("trigger", [app, path, action, text @ ..]) if text.len() <= 1 => {
            let t = t(app)?;
            let at = reach(&t)?;
            // A recording with --markers notes each trigger, where it acted.
            let journal = t.plan.logs.join(video::JOURNAL);
            aio::rt().block_on(async {
                // No --window: the newest (aio: the newest client).
                let n = match value("--window") {
                    Some(_) => window()?,
                    None => cdp::newest(&at).await.unwrap_or(0),
                };
                let mut s = cdp::window_within(&at, n, wait_client()?).await?;
                if journal.exists() {
                    let rect = surface::rect(&mut s, path).await.ok();
                    video::note(&journal, path, action, rect);
                }
                surface::trigger(&mut s, path, action, text.first().copied()).await
            })
        }
        ("timetravel", [app, cell, rest @ ..]) => {
            let (step, to) = live::travel_args(rest.first().copied(), rest.get(1).copied())?;
            live::travel(&door(&t(app)?, cell)?, cell, (step, to), &user(None), json)
        }
        ("heap" | "top", [app, secs @ ..]) if secs.len() <= usize::from(args[0] == "top") => {
            let t = t(app)?;
            let data = &t.plan.data;
            let now = || {
                let health: Option<serde_json::Value> = aio::paths::running(data)
                    .then(|| std::fs::read(data.join("health.json")).ok())
                    .flatten()
                    .and_then(|b| serde_json::from_slice(&b).ok());
                heap(&open(&t.store())?, health.as_ref(), json)
            };
            // `ax top --json`: one view (aio's).
            if (args[0] == "heap" && !has("--watch")) || json {
                return now();
            }
            let every = match secs.first() {
                Some(s) => s
                    .parse::<f64>()
                    .ok()
                    .filter(|s| *s > 0.0 && s.is_finite())
                    .ok_or(format!("{s}: seconds, more than 0"))?,
                None => 2.0,
            };
            // A top view: again every `every` s, until stopped.
            loop {
                println!("{}\n{}\n", aio::log::stamp(now_ms()), now()?);
                std::thread::sleep(std::time::Duration::from_secs_f64(every));
            }
        }
        ("schedules", [app]) => schedules(&open(&t(app)?.store())?, json),
        ("tables", [app]) => sql::tables(&open(&t(app)?.store())?),
        ("sql", [app]) if has("--tables") => sql::tables(&open(&t(app)?.store())?),
        ("sql", [app, words @ ..]) if !words.is_empty() => sql::query(&open(&t(app)?.store())?, &words.join(" ")),
        ("sql", [_]) => Err("ax sql <app> <query>, or --tables".into()),
        ("snapshot", [app]) => snapshot(&t(app)?, value("--as")),
        ("snapshot save", [app]) => snapshot_save(&t(app)?, None, value("--as")),
        ("snapshot save", [app, file]) => snapshot_save(&t(app)?, Some(file), value("--as")),
        ("snapshot load", [app, file]) => {
            let t = t(app)?;
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            // The cells the running app serves: of its store, and of the file.
            let mut cells = open(&t.store()).and_then(|db| metrics::cells(&db)).unwrap_or_default();
            if let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(&text) {
                cells.extend(m.keys().filter(|k| !cells.contains(k)).cloned().collect::<Vec<_>>());
            }
            let served = |c: &String| {
                door(&t, c).is_ok_and(|l| aio::rt().block_on(aio::sync::Live::connect(&l, c, &user(None))).is_ok())
            };
            let app: Vec<String> = cells.into_iter().filter(served).collect();
            if app.is_empty() {
                return Err(format!("{} isn't running, or serves no cell (nothing was loaded)", t.name));
            }
            travel::load(&|cell| door(&t, cell), &text, &user(None), &app, has("--force"))
        }
        ("peer", [app, named @ ..]) => {
            // Another machine's app, by its profile (`ax profile`'s file).
            let remote = app.ends_with(".aioapp") || (Path::new(app).is_file() && !named.is_empty());
            let (cells, name): (Vec<serde_json::Value>, String) = if remote {
                if value("--as").is_some() || value("--role").is_some() {
                    return Err("a profile's caller is its key's: --as and --role are a local app's".into());
                }
                let text = std::fs::read_to_string(app).map_err(|e| format!("{app}: {e}"))?;
                let p: aio::pairing::Profile =
                    serde_json::from_str(&text).map_err(|e| format!("{app}: not a profile: {e}"))?;
                if named.is_empty() {
                    return Err(format!("{app}: name its cells (a server lists none): ax peer {app} <cell>…"));
                }
                let link = aio::sync::Link {
                    addr: format!("{}:{}", p.host.as_deref().unwrap_or("127.0.0.1"), p.port),
                    token: p.key.clone(),
                    root: p.tls.then(|| p.cert.clone()).flatten(),
                };
                let ask = serde_json::json!({ "t": "peer" });
                let cells = named
                    .iter()
                    .map(|c| travel::ask(&link, c, "peer", ask.clone()).map(|(v, _)| v))
                    .collect::<Result<Vec<_>, String>>()?;
                (cells, p.name)
            } else {
                let t = t(app)?;
                let ask = serde_json::json!({ "t": "peer", "user": user(value("--as")), "role": value("--role") });
                // Each with which of its fields are saved and sent (the operator's: this door's).
                let one = |c: &String| {
                    let link = door(&t, c).ok()?;
                    let (mut v, _) = travel::ask(&link, c, &user(None), ask.clone()).ok()?;
                    v["fields"] = travel::ask(&link, c, &user(None), serde_json::json!({ "t": "fields" }))
                        .map_or(serde_json::Value::Null, |(f, _)| f);
                    Some(v)
                };
                let wanted: Vec<String> = match named {
                    [] => metrics::cells(&open(&t.store())?)?,
                    n => n.iter().map(|c| c.to_string()).collect(),
                };
                let cells: Vec<serde_json::Value> = wanted.iter().filter_map(one).collect();
                if cells.is_empty() && !aio::paths::running(&t.plan.data) {
                    return Err(format!("{app} isn't running: ax peer asks the running app"));
                }
                (cells, t.id.clone())
            };
            match value("--types") {
                Some(f) => {
                    let what = format!("{} cells of {name}, their methods by name", cells.len());
                    write_or_say(f, live::peer_types(&name, &cells), &what)
                }
                None => Ok(live::peer(&cells, json)),
            }
        }
        ("cost", [app, cell @ ..]) => {
            let t = t(app)?;
            let cells = match cell.first() {
                Some(c) => vec![c.to_string()],
                None => metrics::cells(&open(&t.store())?)?,
            };
            let window = value("--window").map(window_ms).transpose()?;
            let mut rows = vec![];
            for c in cells {
                let link = door(&t, &c)?;
                match travel::ask(&link, &c, &user(None), serde_json::json!({ "t": "cost", "window": window })) {
                    Ok((meter, _)) => rows.push((c, meter)),
                    // A stored cell the running app serves no door for.
                    Err(_) if cell.is_empty() => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(metrics::cost(&rows, has("--keys"), json))
        }
        ("replay", [app, spec @ ..]) => {
            let t = t(app)?;
            let db = open(&value("--from").map_or_else(|| t.store(), PathBuf::from))?;
            let rows = travel::rows(&db, travel::range(spec.first().copied())?)?;
            let version = |cell: &str| {
                let link = door(&t, cell).ok()?;
                aio::rt().block_on(aio::sync::Live::connect(&link, cell, &user(None))).ok().map(|l| l.version())
            };
            let call =
                |r: &travel::Row| live::call(&door(&t, &r.cell)?, &r.cell, &r.method, r.args.clone(), &user(None));
            travel::replay(&rows, &version, &call, has("--dry"))
        }
        ("record", [app, rest @ ..]) if !rest.is_empty() => {
            let (only, file, spec) = record::args(rest)?;
            if only.is_none() && value("--type").is_some() {
                return Err("--type names one cell's type: ax record <app> <cell> <file> --type path".into());
            }
            let t = t(app)?;
            let store = value("--from").map_or_else(|| t.store(), PathBuf::from);
            let rows: Vec<travel::Row> = travel::rows(&open(&store)?, travel::range(spec.first().copied())?)?
                .into_iter()
                .filter(|r| only.is_none_or(|c| r.cell == c))
                .collect();
            // Per cell: its first and last event in the range.
            let mut span = std::collections::BTreeMap::<String, (u64, u64)>::new();
            for r in &rows {
                span.entry(r.cell.clone()).and_modify(|s| s.1 = r.seq).or_insert((r.seq, r.seq));
            }
            if span.is_empty() {
                let whose = only.map_or_else(|| app.to_string(), String::from);
                return Err(format!("no events of {whose} in that range (ax events {app} lists them)"));
            }
            let ask: std::collections::BTreeMap<String, Vec<u64>> =
                span.iter().map(|(c, &(a, b))| (c.clone(), vec![a, b])).collect();
            let mut at = states_of(&t, &store, value("--from").is_some(), &ask)?;
            // The cells' crate: the app's library (beside its aio.toml), else its package.
            let krate = app_lib(app)
                .map(|(lib, _)| lib)
                .unwrap_or_else(|_| t.pkg.as_ref().map_or(t.id.clone(), |p| p.name.clone()))
                .replace('-', "_");
            // The state before a cell's first event, and the one its last left.
            let states: Vec<(&String, serde_json::Value, serde_json::Value)> = span
                .iter()
                .map(|(c, &(a, b))| {
                    let mut of = at.remove(c).unwrap_or_default();
                    (c, of.remove(&(a - 1)).unwrap_or_default(), of.remove(&b).unwrap_or_default())
                })
                .collect();
            let cells: Vec<record::Recorded> = states
                .iter()
                .map(|(c, before, after)| record::Recorded {
                    id: c,
                    ty: value("--type").map_or_else(|| format!("{krate}::{}", record::type_of(c)), String::from),
                    before,
                    after,
                })
                .collect();
            let text = record::test(app, &record::name_of(file), &cells, &rows, record::exact(&rows));
            if Path::new(file).exists() {
                return Err(format!("{file} exists: pick a new name"));
            }
            if let Some(dir) = Path::new(file).parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(file, text).map_err(|e| format!("{file}: {e}"))?;
            let tys: Vec<&str> = cells.iter().map(|c| c.ty.as_str()).collect();
            Ok(format!(
                "wrote {file}: {} events of {}; its `use` lines must name the cells' types",
                rows.len(),
                tys.join(", ")
            ))
        }
        ("persist", [app]) => {
            let t = t(app)?;
            if !aio::paths::running(&t.plan.data) {
                return Err(format!("{app} isn't running: what it held is written as it stops"));
            }
            let cells = metrics::cells(&open(&t.store())?)?;
            // A cell the running app serves answers its door.
            let ask = |c: &str, msg: serde_json::Value| door(&t, c).and_then(|l| travel::ask(&l, c, &user(None), msg));
            match value("--drop") {
                Some(gone) => {
                    if ask(gone, serde_json::json!({ "t": "fields" })).is_ok() {
                        return Err(format!(
                            "{gone} is a cell of the running app: only a stored cell it no longer declares is dropped"
                        ));
                    }
                    if !cells.iter().any(|c| c == gone) {
                        return Err(format!("{gone}: no such cell in the store (ax migrations lists them)"));
                    }
                    let mut why = format!("{app} serves no cell to ask");
                    for c in cells.iter().filter(|c| *c != gone) {
                        match ask(c, serde_json::json!({ "t": "drop", "cell": gone })) {
                            Ok(_) => return Ok(format!("dropped {gone}: its history is gone from the store")),
                            Err(e) if e.contains("isn't running") || e.contains("os error") => why = e,
                            Err(e) => return Err(e),
                        }
                    }
                    Err(why)
                }
                None => {
                    let done: Vec<String> = cells
                        .iter()
                        .filter_map(|c| {
                            ask(c, serde_json::json!({ "t": "flush" }))
                                .ok()
                                .map(|(seq, _)| format!("{c}: written (event {seq})"))
                        })
                        .collect();
                    if done.is_empty() {
                        return Err(format!("{app} serves none of its stored cells"));
                    }
                    Ok(done.join("\n"))
                }
            }
        }
        ("migrations", [app]) => {
            let t = t(app)?;
            let shape = |cell: &str| {
                let link = door(&t, cell).ok()?;
                travel::ask(&link, cell, &user(None), serde_json::json!({ "t": "shape" })).ok().map(|(v, _)| v)
            };
            travel::migrations(&open(&t.store())?, &shape, json)
        }
        ("trust", sub) => trust::run(&aio::paths::tool_home().join("tls"), sub.first().copied(), json),
        ("theme adopt", [app]) => theme::run(app, has("--force")),
        ("prune", []) => {
            let f: Vec<&str> = flags.iter().map(String::as_str).collect();
            aio_shell::runtime::migrate();
            let said = prune::run(&aio_shell::runtime::root(), &f, std::time::SystemTime::now())?;
            let yes = has("--yes") || has("-y");
            match prune::legacy(&upgrade::legacy_target(&aio::paths::tool_home()), yes)? {
                Some(old) => Ok(format!("{said}\n{old}")),
                None => Ok(said),
            }
        }
        ("runtime", []) => runtime(has("--fetch")),
        ("setup", []) if has("--warm") => {
            let said = setup::run(has("--check"))?;
            Ok(format!("{said}\n{}", setup_warm(value("--aio").map(Path::new))?))
        }
        ("setup", []) => {
            let said = setup::run(has("--check"))?;
            // A first app's start is a launch, not aio-rs's 20 s build: built
            // now, before setup returns (the installers run `ax setup`), so
            // no first `ax dev` waits on it or races it. Ctrl-C leaves the
            // rest to the first build.
            let terminal = std::io::IsTerminal::is_terminal(&std::io::stderr()) && !json;
            let env = std::env::var("AIORT_NO_WARM").ok();
            if !setup_warms(has("--check"), terminal, env.as_deref()) {
                return Ok(said);
            }
            eprintln!("{said}\nwarm: building aio-rs for ax dev, once per machine (AIORT_NO_WARM=1: not)…");
            Ok(setup_warm(None).unwrap_or_else(|e| format!("{e}; the first ax dev builds it")))
        }
        ("pair", [app]) => pair_code(&t(app)?, has("--plain")),
        ("pair", [app, addr, code]) => pair(&t(app)?, addr, code),
        ("pair", [_, addr]) => {
            Err(format!("ax pair <app> {addr} <code>: the code too (ax pair <app> on the app's machine makes one)"))
        }
        ("auth", [app, what @ ..]) => {
            let f = auth::Fields { role: value("--role"), email: value("--email"), password: value("--password") };
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
            auth::run(&t(app)?.plan.data, what, &f, json, now as i64)
        }
        ("doctor", [app]) if has("--harden") => {
            let t = t(app)?;
            let file = t
                .pkg
                .as_ref()
                .map(build::toml_of)
                .transpose()?
                .ok_or(format!("{app}: --harden writes the app's aio.toml: run it in its workspace"))?;
            let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            match doctor::harden(&text) {
                None => Ok(format!("{}: every security switch is decided; nothing to write", file.display())),
                Some((said, new)) => {
                    std::fs::write(&file, new).map_err(|e| format!("{}: {e}", file.display()))?;
                    Ok(std::iter::once(format!("wrote {} (build it again to take it in):", file.display()))
                        .chain(said.iter().map(|l| format!("  {l}")))
                        .collect::<Vec<_>>()
                        .join("\n"))
                }
            }
        }
        ("doctor", [app]) => {
            let t = t(app)?;
            let health = aio::paths::running(&t.plan.data)
                .then(|| std::fs::read(t.plan.data.join("health.json")).ok())
                .flatten()
                .and_then(|b| serde_json::from_slice(&b).ok());
            let ms = |p: &Path| {
                let at = std::fs::metadata(p).and_then(|m| m.modified()).ok()?;
                at.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
            };
            let toml = t.pkg.as_ref().and_then(|p| build::toml_of(p).ok());
            let text = toml.as_ref().and_then(|f| std::fs::read_to_string(f).ok());
            let open = text
                .as_deref()
                .and_then(|s| aio::config::Config::parse(s).ok())
                .filter(|c| c.server.open)
                .map(|c| c.app.id);
            let unhardened = text.as_deref().and_then(doctor::harden).map(|(said, _)| said).unwrap_or_default();
            // The binary it runs: the start's record, else its build, else the installed one.
            let bin = start::record(&t.plan.logs)
                .and_then(|r| r["bin"].as_str().map(PathBuf::from))
                .or_else(|| t.pkg.as_ref().map(|p| p.bin.clone()))
                .unwrap_or_else(|| places().bin.join(&t.id));
            let runtime = start::windowed(&t)
                .then(|| std::fs::read(&bin).ok())
                .flatten()
                .and_then(|b| aio_shell::runtime::pinned_in(&b));
            let config = match toml {
                Some(f) => format!("{} (built in)", f.display()),
                None => "built into the app (ax doctor in its workspace says from where)".into(),
            };
            let facts = doctor::Facts {
                health,
                source: t.pkg.as_ref().map(|p| (p.version.clone(), doctor::newest(&p.dirs), ms(&p.bin))),
                config,
                dirs: (t.plan.from.to_string(), t.plan.data.clone()),
                env: doctor::VARS
                    .iter()
                    .filter_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()).map(|v| (k.to_string(), v)))
                    .collect(),
                open,
                patched: std::fs::read_to_string(t.plan.logs.join(dev::PATCHED))
                    .ok()
                    .and_then(|s| s.trim().parse().ok()),
                runtime,
                unhardened,
            };
            match json {
                true => {
                    let (findings, said) = doctor::verdict(&facts);
                    Ok(serde_json::json!({ "app": t.name, "running": facts.health.is_some(), "findings": findings, "said": said }).to_string())
                }
                false => Ok(doctor::render(&facts)),
            }
        }
        ("profile", [app]) => profile(&t(app)?, value("--out"), has("--link")),
        ("import", [app, file]) => {
            let t = t(app)?;
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            let addr = aio::pairing::import(&t.plan.data, &t.id, &text).map_err(|e| e.to_string())?;
            Ok(format!("linked {} to {addr}", t.name))
        }
        ("backup", [app]) => backup(&t(app)?, None, json),
        ("backup", [app, file]) => backup(&t(app)?, Some(file), json),
        ("restore", [app, file]) => restore(&t(app)?, file),
        // `verbs::read` lets through only what a verb's usage allows.
        _ => Err(format!("no way to run: ax {}", args.join(" "))),
    }
}

/// The app of the folder the command runs in.
fn here() -> Result<String, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let meta = dev::metadata().map_err(|_| "no app here (no Cargo workspace): give one, or --app <name>")?;
    dev::here(&meta, &cwd)
}

/// The answer to a command line as `verbs::read` read it: its exit code
/// (0 ok, 1 failed, 2 bad usage) and what to print.
fn outcome(asked: Result<verbs::Ask, String>) -> (u8, Out) {
    match asked {
        Ok(verbs::Ask::Say(text)) => (0, Ok(text)),
        Ok(verbs::Ask::Run(args)) => match run(&args) {
            Ok(out) => (0, Ok(out)),
            Err(e) => (1, Err(e)),
        },
        Err(e) => (2, Err(e)),
    }
}

/// `--json`: one object, `{"ok":true,"value":…}` for an answer that is JSON
/// (a state, a call's answer), `{"ok":true,"text":…}` for one that isn't,
/// `{"ok":false,"error":…,"exit":n}` for a failure.
fn as_json(code: u8, out: &Out) -> String {
    let v = match out {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(text) {
            Ok(v) => serde_json::json!({ "ok": true, "value": v }),
            Err(_) => serde_json::json!({ "ok": true, "text": text }),
        },
        // A verb's failure that is an object (`ax health`, `ax expect`) comes whole.
        Err(e) => match serde_json::from_str::<serde_json::Value>(e) {
            Ok(v @ serde_json::Value::Object(_)) => {
                serde_json::json!({ "ok": false, "error": v.get("error").cloned().unwrap_or("failed".into()), "exit": code, "value": v })
            }
            _ => serde_json::json!({ "ok": false, "error": e, "exit": code }),
        },
    };
    v.to_string()
}

/// The verbs whose answer is a document or a stream, never wrapped: an
/// agent brief, a test, a type table, a state to load back, log lines.
const DOCUMENTS: [&str; 10] =
    ["dev", "lint", "agent", "testgen", "record", "peer", "snapshot", "logs", "help", "version"];

/// Whether ax answers in JSON though `--json` wasn't typed (aio's `am`: a
/// pipe is read by a program): its stdout isn't a terminal, or
/// `AIORT_OUTPUT=json`; `--text` or `AIORT_OUTPUT=text` keeps the text
/// (pure). A document verb and help or version are never wrapped.
fn piped_json(args: &[String], terminal: bool, output: Option<&str>) -> bool {
    let Some(verb) = args.first() else { return false };
    let flags = || args.iter().take_while(|a| *a != "--");
    let asked = flags().any(|a| ["--json", "--text", "--help", "-h", "--version", "-V"].contains(&a.as_str()));
    if asked || DOCUMENTS.contains(&verb.as_str()) || verb.starts_with('-') {
        return false;
    }
    match output {
        Some("text") => false,
        Some("json") => true,
        _ => !terminal,
    }
}

/// Run beside an app's folder (`ax dev notes` where `notes/` is, in no
/// workspace): the app named that folder, as if after `cd notes` (pure:
/// which folder, if any). Only the verbs on an app's source; `in_workspace`:
/// a Cargo.toml here or above, so nothing to step into.
fn steps_in<'a>(args: &'a [String], in_workspace: bool, has_manifest: &dyn Fn(&str) -> bool) -> Option<&'a str> {
    const SOURCE: [&str; 10] =
        ["dev", "test", "check", "build", "release", "keygen", "preview", "config", "lab", "install"];
    let (verb, app) = (args.first()?, args.get(1)?);
    let plain = !app.starts_with('-') && !app.contains(['/', '\\', '.']);
    (SOURCE.contains(&verb.as_str()) && plain && !in_workspace && has_manifest(app)).then_some(app.as_str())
}

fn main() -> ExitCode {
    // ax as its own hot-patch build's rustc wrapper or linker: first, as little as can be.
    if let Some(code) = hotpatch::hook() {
        return code;
    }
    // ax as rustc's wrapper for `ax dev --warm --pack`: this machine's paths remapped.
    if let Some(code) = prebuilt::hook() {
        return code;
    }
    // ax as a macOS cross build's C compiler and linker (zig's).
    if let Some(code) = zig::hook() {
        return code;
    }
    _ = fetched::PERSONS_TOOLCHAIN.set(std::env::var_os("RUSTUP_TOOLCHAIN"));
    // Every cargo ax runs builds into one folder per machine and toolchain.
    // SAFETY: before any thread is started.
    unsafe { dev::keep_persons_target() };
    // SAFETY: before any thread is started.
    let shared = || unsafe { dev::set_shared_target() };
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Its own exit codes (aio's aiol): 1 errors found, 2 bad usage.
    if args.first().map(String::as_str) == Some("lint") {
        shared();
        return lint::main(&args[1..]);
    }
    let mut args = args;
    let output = std::env::var("AIORT_OUTPUT").ok();
    if piped_json(&args, std::io::IsTerminal::is_terminal(&std::io::stdout()), output.as_deref()) {
        args.insert(1, "--json".into());
    }
    let json = args.first().map(String::as_str) != Some("dev")
        && args.iter().take_while(|a| *a != "--").any(|a| a == "--json");
    if let Ok(cwd) = std::env::current_dir()
        && let Some(app) = steps_in(&args, cwd.ancestors().any(|d| d.join("Cargo.toml").is_file()), &|a| {
            cwd.join(a).join("Cargo.toml").is_file()
        })
        && std::env::set_current_dir(cwd.join(app)).is_ok()
    {
        eprintln!("ax: no workspace here, but ./{app} is one: in it, as after `cd {app}`");
    }
    // Here, in the app's folder: its toolchain names the folder.
    shared();
    let asked = verbs::read(&args, &here);
    // Its own exit codes (aio's): 0 started, 1 stopped, 2 on the way.
    if let Ok(verbs::Ask::Run(a)) = &asked
        && a[0] == "status"
    {
        let cut = a.iter().position(|w| w == "--").unwrap_or(a.len());
        let names: Vec<&str> = a[cut + 1..].iter().map(String::as_str).collect();
        return match aio::paths::Ask::parse(&a[1..cut]) {
            Ok(ask) => status::main(&names, &ask, json),
            Err(e) => {
                eprintln!("ax: {e}");
                ExitCode::from(2)
            }
        };
    }
    let (code, out) = outcome(asked);
    // aio's `--quiet`: the exit code says it; a failure still says why.
    let quiet =
        args.iter().take_while(|a| *a != "--").any(|a| a == "--quiet") && args.first().is_some_and(|a| a != "dev");
    match (&out, json) {
        (_, true) => println!("{}", as_json(code, &out)),
        (Ok(_), false) if quiet => {}
        (Ok(text), false) if !text.is_empty() => println!("{text}"),
        (Ok(_), false) => {}
        (Err(e), false) => eprintln!("ax: {e}"),
    }
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aio::prelude::*;

    /// A method's and a server function's arguments read the same: words, `--args` or `--body`.
    #[test]
    fn a_calls_body_is_its_one_object_and_nothing_with_it() {
        let j = aio::serde_json::json!([{ "items": [1, 2] }]);
        assert_eq!(call_body(&[], None, Some(r#"{"items":[1,2]}"#)), Ok(j.clone()));
        assert_eq!(call_body(&["items=[1,2]"], None, None), Ok(j));
        assert!(call_body(&["x"], None, Some("{}")).unwrap_err().contains("no words or --args"));
        assert!(call_body(&[], Some("[]"), Some("{}")).unwrap_err().contains("no words or --args"));
    }

    #[test]
    fn the_share_link_carries_the_key_encoded() {
        let p = aio::serde_json::json!({ "host": "192.0.2.7", "port": 8443, "tls": true, "key": "ab12" });
        assert_eq!(share_link(&p), "https://192.0.2.7:8443/?token=ab12");
        let p = aio::serde_json::json!({ "host": "::1", "port": 80, "tls": false, "key": "a b&c" });
        assert_eq!(share_link(&p), "http://[::1]:80/?token=a%20b%26c");
    }

    #[test]
    fn the_build_version_is_the_flags_then_aio_rs_then_aios_variable() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(build_version(Some("1.0.0"), s("2.0.0"), s("3.0.0")), s("1.0.0"));
        assert_eq!(build_version(None, s("2.0.0"), s("3.0.0")), s("2.0.0"));
        assert_eq!(build_version(None, None, s("3.0.0")), s("3.0.0"), "aio's AIO_BUILD_VERSION");
        assert_eq!(build_version(None, None, None), None);
    }

    #[test]
    fn a_pipe_gets_json_unless_text_is_asked_and_documents_never() {
        let a = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert!(piped_json(&a("state notes counter"), false, None), "a pipe: aio's am");
        assert!(!piped_json(&a("state notes counter"), true, None), "a terminal: text");
        assert!(piped_json(&a("state notes counter"), true, Some("json")));
        assert!(!piped_json(&a("state notes counter"), false, Some("text")));
        assert!(!piped_json(&a("state notes counter --text"), false, None));
        assert!(!piped_json(&a("state notes counter --json"), false, None), "asked already");
        assert!(!piped_json(&a("call notes c m -- --text"), true, Some("text")));
        assert!(piped_json(&a("call notes c m -- --text"), false, None), "an arg, not ax's flag");
        for doc in ["agent", "snapshot notes", "logs notes", "dev notes", "lint notes", "help"] {
            assert!(!piped_json(&a(doc), false, Some("json")), "{doc}");
        }
        assert!(!piped_json(&a("--version"), false, None) && !piped_json(&a("version --help"), false, None));
    }

    #[test]
    fn ax_start_builds_as_release_runs_unless_devtools_are_asked() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(!asks_dev(&v(&[])), "no door, no DevTools port, hardened");
        assert!(!asks_dev(&v(&["--seed", "demo"])));
        assert!(!asks_dev(&v(&["--", "--cdp"])), "the app's own word");
        assert!(asks_dev(&v(&["--cdp"])) && asks_dev(&v(&["--cdp=9333"])));
    }

    #[test]
    fn ax_setup_at_a_terminal_builds_aio_rs_in_the_background() {
        assert!(setup_warms(false, true, None), "the installer's ax setup, a person watching");
        assert!(!setup_warms(true, true, None), "--check only looks");
        assert!(!setup_warms(false, false, None), "CI, a script, an agent");
        assert!(!setup_warms(false, true, Some("1")), "AIORT_NO_WARM=1");
    }

    #[test]
    fn the_warm_scratch_app_is_made_again_by_another_ax_or_pin() {
        let d = tempfile::tempdir().unwrap();
        let stamp = "0.3.0 1.98.0 Tag(\"v0.3.0\")\n";
        assert!(!scratch_current(d.path(), stamp), "none yet");
        std::fs::write(d.path().join("Cargo.toml"), "").unwrap();
        assert!(!scratch_current(d.path(), stamp), "one an older ax made, unstamped");
        std::fs::write(d.path().join(WARM_STAMP), stamp).unwrap();
        assert!(scratch_current(d.path(), stamp));
        assert!(!scratch_current(d.path(), "0.3.1 1.98.0 Tag(\"v0.3.1\")\n"), "a newer ax");
        assert!(!scratch_current(d.path(), "0.3.0 1.99.0 Tag(\"v0.3.0\")\n"), "another toolchain pin");
    }

    #[cell(persist)]
    pub struct Notes {
        pub text: String,
    }

    #[methods]
    impl Notes {
        pub fn set(&mut self, cx: &Cx, t: String) -> Result {
            self.text = t;
            cx.every("tick", 60_000, "set", ("tick",))
        }

        #[redact]
        pub fn seal(&mut self, secret: String) {
            self.text = format!("sealed {}", secret.len());
        }
    }

    #[test]
    fn a_file_or_dash_for_stdout() {
        assert_eq!(write_or_say("-", "pub mod notes {}".into(), "x"), Ok("pub mod notes {}".into()));
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("peer.rs");
        assert_eq!(
            write_or_say(f.to_str().unwrap(), "t".into(), "1 cell"),
            Ok(format!("wrote {}: 1 cell", f.display()))
        );
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "t");
    }

    #[test]
    fn says_plainly_why_an_app_serves_no_device() {
        assert_eq!(not_serving("notes", false, None), "notes isn't running: start it with `ax start notes`");
        assert_eq!(not_serving("notes", false, Some(true)), "notes isn't running: start it with `ax start notes`");
        let none = not_serving("notes", false, Some(false));
        assert!(none.starts_with("notes isn't running, and has no server for other devices: give it one in its aio.toml ([app] mode = \"client-server\", [server] listen = "), "{none}");
        assert!(none.ends_with("then `ax start notes`"));
        assert!(not_serving("notes", true, Some(false)).ends_with("then `ax restart notes`"));
        for m in [none, not_serving("notes", true, None)] {
            assert!(!m.contains("serving.json"), "{m}");
        }
    }

    #[test]
    fn beside_an_apps_folder_a_source_verb_runs_in_it() {
        let a = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let notes = |d: &str| d == "notes";
        assert_eq!(steps_in(&a("dev notes --display isolated"), false, &notes), Some("notes"));
        assert_eq!(steps_in(&a("test notes"), true, &notes), None, "in a workspace: as it is");
        assert_eq!(steps_in(&a("create notes"), false, &notes), None, "create makes the folder");
        assert_eq!(steps_in(&a("state notes counter"), false, &notes), None, "a running app's verbs need no source");
        assert_eq!(steps_in(&a("build other"), false, &notes), None, "no such folder");
        assert_eq!(steps_in(&a("dev ../notes"), false, &|_| true), None, "a name, not a path");
    }

    #[test]
    fn a_redacted_methods_arguments_are_never_shown() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state.db");
        let h = Handle::<Notes>::open(&p).unwrap();
        h.seal("hunter2-passphrase".into()).unwrap();
        drop(h);
        let db = open(&p).unwrap();
        let ev = events(&db, None, 20, false, &|_| vec![]).unwrap();
        assert!(ev.contains("notes.seal([redacted])") && !ev.contains("hunter2"), "{ev}");
        let rows: serde_json::Value = serde_json::from_str(&events(&db, None, 20, true, &|_| vec![]).unwrap()).unwrap();
        assert_eq!(rows[0]["args"], "[redacted]");
    }

    fn store() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state.db");
        let h = Handle::<Notes>::open(&p).unwrap();
        h.call("set", serde_json::json!(["hello"])).unwrap();
        h.call("set", serde_json::json!(["x".repeat(500)])).unwrap();
        drop(h);
        (d, p)
    }

    #[test]
    fn a_redacted_call_shows_as_no_more_than_that() {
        let (_d, p) = store();
        Connection::open(&p)
            .unwrap()
            .execute(
            "INSERT INTO log (cell, method, args, inputs, by, at, v) VALUES ('notes', 'set', ?1, '{}', '\"system\"', 1, 1)",
            [serde_json::json!({ aio::REDACTED: { "text": "made" } }).to_string()],
        )
        .unwrap();
        let db = open(&p).unwrap();
        let ev = events(&db, None, 1, false, &|_| vec![]).unwrap();
        assert!(ev.contains("notes.set([redacted])") && !ev.contains("made"), "{ev}");
        let rows: serde_json::Value = serde_json::from_str(&events(&db, None, 1, true, &|_| vec![]).unwrap()).unwrap();
        assert_eq!(rows[0]["args"], "[redacted]");
    }

    #[test]
    fn shows_events_state_and_schedules() {
        let (_d, p) = store();
        let db = open(&p).unwrap();
        let ev = events(&db, Some("notes"), 20, false, &|_| vec![]).unwrap();
        assert!(ev.contains("notes.set([\"hello\"])"), "{ev}");
        assert!(ev.contains("bytes)"), "long args are cut: {ev}");
        assert_eq!(events(&db, None, 1, false, &|_| vec![]).unwrap().lines().count(), 1);
        let rows: serde_json::Value = serde_json::from_str(&events(&db, None, 20, true, &|_| vec![]).unwrap()).unwrap();
        assert_eq!((rows[0]["method"].as_str(), &rows[0]["args"]), (Some("set"), &serde_json::json!(["hello"])));
        assert!(rows[0].get("diff").is_none());
        // --diff: each event with the changes it made, under it.
        let diff = |seq: i64| vec![("text".to_string(), serde_json::json!(seq - 1), serde_json::json!(seq))];
        let ev = events(&db, Some("notes"), 1, false, &diff).unwrap();
        assert_eq!(ev.lines().nth(1), Some("          text: 1 → 2"), "{ev}");
        let rows: serde_json::Value = serde_json::from_str(&events(&db, None, 1, true, &diff).unwrap()).unwrap();
        assert_eq!(rows[0]["diff"], serde_json::json!([{ "path": "text", "before": 1, "after": 2 }]));
        let rows: serde_json::Value = serde_json::from_str(&schedules(&db, true).unwrap()).unwrap();
        assert_eq!((rows[0]["key"].as_str(), rows[0]["every"].as_i64()), (Some("tick"), Some(60_000)));
        let h: serde_json::Value = serde_json::from_str(&heap(&db, None, true).unwrap()).unwrap();
        assert_eq!(
            (h["memory"].is_null(), h["cells"][0]["cell"].as_str(), h["cells"][0]["events"].as_i64()),
            (true, Some("notes"), Some(2))
        );
        let (head, st) = stored(&db, "notes").unwrap();
        assert!(head.starts_with("notes v1 after event 2"), "{head}");
        assert_eq!(st["text"].as_str().map(str::len), Some(500));
        assert!(schedules(&db, false).unwrap().contains("notes:tick → set every 60000 ms"));
        assert!(stored(&db, "nope").is_err());
        let h = heap(&db, None, false).unwrap();
        let lines: Vec<&str> = h.lines().collect();
        assert_eq!(lines[0], "not running: its stored cells only");
        assert!(lines[1].contains("cell") && lines[1].contains("event args"), "{h}");
        assert!(lines[2].starts_with("  notes ") && lines[2].contains(" 2 "), "{h}");
        let mem = serde_json::json!({ "memory": { "rssBytes": 50u64 << 20, "machinePct": 1, "level": "ok" } });
        assert_eq!(heap(&db, Some(&mem), false).unwrap().lines().next(), Some("50 MB resident (1% of RAM), memory ok"));
    }

    #[test]
    fn a_backup_goes_beside_the_data_unless_named() {
        let f = backup_file("notes", Path::new("/h/notes/data/state.db"), 0);
        assert_eq!(f, Path::new("/h/notes/backups/notes-1970-01-01T00-00-00-000Z.aio-backup"));
    }

    #[test]
    fn errors_come_by_start_with_the_failed_build_first() {
        let log = [
            "t0 INFO  [x] early",
            "t0 WARN  [x] before any start",
            "t1 [aio] notes 2.0.0 starting (pid 10)",
            "t2 ERROR [x] one",
            "t2 INFO  [x] fine",
            "t3 [aio] notes 2.0.0 starting (pid 11)",
            "t4 thread 'main' panicked at src/lib.rs:3:5",
            "t5 WARN  [x] two",
        ];
        let all = errors(None, &log, 20, false);
        assert_eq!(
            all.lines().collect::<Vec<_>>(),
            [
                "before the first start in the log:",
                "  t0 WARN  [x] before any start",
                "start 1: t1 [aio] notes 2.0.0 starting (pid 10)",
                "  t2 ERROR [x] one",
                "start 2: t3 [aio] notes 2.0.0 starting (pid 11)",
                "  t4 thread 'main' panicked at src/lib.rs:3:5",
                "  t5 WARN  [x] two",
            ]
        );
        // A hot-patch is new code: what it says is under it, not its start.
        let mut patched = log.to_vec();
        patched.insert(6, "t3 [aio] hot-patch 1 applied (12 functions)");
        let out = errors(None, &patched, 20, false);
        assert!(out.ends_with("patch 3: t3 [aio] hot-patch 1 applied (12 functions)\n  t4 thread 'main' panicked at src/lib.rs:3:5\n  t5 WARN  [x] two"), "{out}");
        // The newest n, still under their start.
        assert_eq!(errors(None, &log, 1, false), "start 2: t3 [aio] notes 2.0.0 starting (pid 11)\n  t5 WARN  [x] two");
        assert!(errors(None, &log[..1], 20, false).starts_with("no warnings or errors in the log (1 lines)"));
        let b = dev::build_log(0, "error[E0425]: cannot find value `x`\n --> src/lib.rs:1:1");
        let out = errors(Some(&b), &[], 20, false);
        assert!(out.starts_with(
            "build failed at 1970-01-01T00:00:00.000Z (the app runs its last good build)\n  error[E0425]"
        ));
    }

    #[test]
    fn errors_since_a_time_or_the_newest_start_and_as_json() {
        let log = [
            "2026-10-28T10:00:00.000Z [aio] notes 1 starting (pid 10)",
            "2026-10-28T10:01:00.000Z ERROR [x] old",
            "2026-10-28T11:00:00.000Z [aio] notes 1 starting (pid 11)",
            "2026-10-28T11:05:00.000Z WARN  [x] before the patch",
            "2026-10-28T11:10:00.000Z ERROR [x] after the patch",
        ];
        let at = |s: &str| logs::since(s, 0).unwrap();
        assert_eq!(newer(&log, Some(at("2026-10-28T11:00:00Z")), None), &log[2..]);
        assert_eq!(newer(&log, None, Some(None)), &log[2..], "reload: since the newest start");
        assert_eq!(newer(&log, None, Some(Some(at("2026-10-28T11:06:00Z")))), &log[4..], "or the hot patch after it");
        assert_eq!(
            newer(&log, None, Some(Some(at("2026-10-28T09:00:00Z")))),
            &log[2..],
            "a patch before it: the start"
        );
        assert!(newer(&log, Some(at("2027-01-01T00:00:00Z")), None).is_empty());
        let v: serde_json::Value =
            serde_json::from_str(&errors(Some("build failed\nE"), newer(&log, None, Some(None)), 20, true)).unwrap();
        assert_eq!(
            (v["total"].as_u64(), v["build"].as_str(), v["read"].as_u64()),
            (Some(2), Some("build failed\nE"), Some(3))
        );
        assert_eq!(v["starts"][0]["lines"][1], log[4]);
        assert_eq!(v["starts"][0]["line"], log[2]);
    }

    /// amui's structured errors: a panic's lines are one error, never cut
    /// to its first.
    #[test]
    fn a_panic_is_one_error_with_all_its_lines() {
        let log = [
            "2026-10-28T10:00:00.000Z [aio] notes 1 starting (pid 10)",
            "2026-10-28T10:01:00.000Z ERROR [aio] panicked at src/lib.rs:3:5",
            "2026-10-28T10:01:00.000Z   index out of bounds",
            "2026-10-28T10:01:01.000Z INFO  [x] fine",
            "2026-10-28T10:01:02.000Z   not a panic's",
        ];
        let v: serde_json::Value = serde_json::from_str(&errors(None, &log, 20, true)).unwrap();
        assert_eq!(v["total"], 1);
        assert_eq!(v["starts"][0]["lines"][0], format!("{}\n{}", log[1], log[2]));
        let text = errors(None, &log, 20, false);
        assert!(text.ends_with(&format!("  {}\n  {}", log[1], log[2])), "{text}");
    }

    /// amui's per-action reduce time and errors: each event with its
    /// method's time, the refused calls as rows of their own, by time.
    #[test]
    fn events_carry_their_time_and_the_refused_calls_join_them() {
        let row = |seq: i64, at: i64| serde_json::json!({ "seq": seq, "cell": "board", "method": "add", "at": at });
        let read = [(
            "board".to_string(),
            serde_json::json!({ "ms": { "7": 0.5 }, "refused": [
                { "at": "2026-10-28T10:00:01.000Z", "call": "board.clear", "ms": 0.1, "refused": "only an admin" },
                { "at": "2026-10-28T10:00:01.000Z", "call": "boardx.clear", "ms": 0.1, "refused": "another cell's" },
            ] }),
        )];
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-28T10:00:00Z").unwrap().timestamp_millis();
        let mut rows = vec![row(7, at), row(8, at + 5_000)];
        timed(&mut rows, &read, false);
        assert_eq!((rows.len(), &rows[0]["ms"], rows[1].get("ms")), (2, &serde_json::json!(0.5), None));
        timed(&mut rows, &read, true);
        assert_eq!(rows.len(), 3, "board's refused call only");
        assert_eq!(
            (&rows[1]["method"], &rows[1]["refused"], rows[1].get("seq")),
            (&"clear".into(), &"only an admin".into(), None)
        );
        assert_eq!(rows[2]["seq"], 8, "by time");
    }

    #[test]
    fn a_cost_window_is_seconds_or_a_unit_an_hour_at_most() {
        assert_eq!(window_ms("90"), Ok(90_000));
        assert_eq!(window_ms("5m"), Ok(300_000));
        assert_eq!(window_ms("1h"), Ok(3_600_000));
        assert!(window_ms("2h").unwrap_err().contains("an hour at most"));
        assert!(window_ms("0").is_err() && window_ms("soon").unwrap_err().contains("30s, 5m"));
    }

    #[test]
    fn a_failure_that_is_an_object_comes_whole_and_rows_say_the_first_error() {
        let fail = serde_json::json!({ "healthy": false, "error": "stuck" }).to_string();
        let v: serde_json::Value = serde_json::from_str(&as_json(1, &Err(fail))).unwrap();
        assert_eq!(
            (v["ok"].as_bool(), v["error"].as_str(), v["value"]["healthy"].as_bool()),
            (Some(false), Some("stuck"), Some(false))
        );
        assert_eq!(failed_or(vec![serde_json::json!({ "app": "a" })]), r#"[{"app":"a"}]"#);
        let one = failed_or(vec![serde_json::json!({ "app": "a" }), serde_json::json!({ "app": "b", "error": "no" })]);
        assert!(one.starts_with(r#"{"apps":"#) && one.contains(r#""error":"no""#), "{one}");
    }

    #[test]
    fn logs_read_across_rotated_files() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("app.1.log"), "a\nb\n").unwrap();
        std::fs::write(d.path().join("app.log"), "c\n").unwrap();
        assert_eq!(log_lines(d.path(), 2).unwrap(), "b\nc");
        assert_eq!(log_lines(d.path(), 50).unwrap(), "a\nb\nc");
        assert!(log_lines(&d.path().join("none"), 5).is_err());
    }

    #[test]
    fn refuses_what_isnt_a_store_and_bad_commands() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("other.db");
        Connection::open(&p).unwrap().execute_batch("CREATE TABLE t (x)").unwrap();
        assert!(open(&p).unwrap_err().contains("not an aio-rs store"));
        assert!(open(&d.path().join("missing.db")).is_err());
        assert_eq!(cli("frobnicate").0, 2);
        assert_eq!(cli("events a --last").0, 2);
        assert!(cli("events a --last x").1.unwrap_err().contains("--last needs a number"));
        assert!(cli("").1.unwrap().contains("ax state"));
    }

    fn cli(line: &str) -> (u8, Out) {
        outcome(verbs::read(&line.split(' ').filter(|w| !w.is_empty()).map(String::from).collect::<Vec<_>>(), &here))
    }

    #[test]
    fn json_is_one_object_with_the_exit_code() {
        assert_eq!(as_json(0, &Ok("{\"a\":1}".into())), r#"{"ok":true,"value":{"a":1}}"#);
        assert_eq!(as_json(0, &Ok("3 links".into())), r#"{"ok":true,"text":"3 links"}"#);
        assert_eq!(as_json(2, &Err("bad".into())), r#"{"error":"bad","exit":2,"ok":false}"#);
    }

    #[test]
    fn the_config_as_it_runs_has_every_default() {
        let out = effective(Path::new("/w/aio.toml"), "[app]\nid = \"notes\"\n[history]\nkeep = \"7d\"").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!((v["from"].as_str(), v["config"]["app"]["id"].as_str()), (Some("/w/aio.toml"), Some("notes")));
        assert_eq!(
            (v["config"]["history"]["keep"].as_str(), v["config"]["updates"]["channel"].as_str()),
            (Some("7d"), Some("stable"))
        );
        assert_eq!(v["config"]["app"]["mode"], "standalone");
        let bad = effective(Path::new("/w/aio.toml"), "[app]\nid = \"notes\"\nnope = 1").unwrap_err();
        assert!(bad.starts_with("/w/aio.toml: aio.toml: ") && bad.contains("nope"), "{bad}");
    }

    #[test]
    fn a_target_is_an_app_where_a_launch_would_run_it() {
        let none = aio::paths::Ask::default();
        let t = target::of("no-such-app-0a1b@work", &none).unwrap();
        assert_eq!((t.id.as_str(), t.plan.profile.as_deref()), ("no-such-app-0a1b", Some("work")));
        assert!(t.store().ends_with("data/state.db") && t.plan.from != "default", "{:?}", t.plan);
        let inst = aio::paths::Ask { instance: Some("agent1".into()), ..none.clone() };
        let t = target::of("no-such-app-0a1b", &inst).unwrap();
        assert_eq!(t.plan.from, "--instance");
        assert!(t.plan.data.ends_with("instances/agent1/no-such-app-0a1b/data"), "{:?}", t.plan.data);
        let both = aio::paths::Ask { profile: Some("x".into()), ..none };
        assert!(target::of("a@y", &both).unwrap_err().contains("both name a profile"));
    }

    #[test]
    fn a_report_is_filed_for_a_stopped_app_from_its_log_without_state() {
        let d = tempfile::tempdir().unwrap();
        let home = aio::paths::Ask { home: Some(d.path().display().to_string()), ..Default::default() };
        let t = target::of("no-such-app-0a1b", &home).unwrap();
        std::fs::create_dir_all(&t.plan.logs).unwrap();
        std::fs::write(t.plan.logs.join("app.log"), "2026-11-08T10:00:00.000Z ERROR boom token=s3cret\n").unwrap();
        let said: serde_json::Value = serde_json::from_str(&new_report(&t, "it froze", true).unwrap()).unwrap();
        let file = PathBuf::from(said["file"].as_str().unwrap());
        assert!(file.starts_with(t.plan.data.join("reports")), "{file:?}");
        let r: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!((r["title"].as_str(), r["kind"].as_str()), (Some("it froze"), Some("user")));
        assert_eq!(r["state"], serde_json::json!({}), "no state of a stopped app");
        let logs = r["logs"].to_string();
        assert!(logs.contains("ERROR boom") && !logs.contains("s3cret"), "masked: {logs}");
        let listed = report(&t.plan.data, None, false).unwrap();
        assert!(listed.contains("it froze"), "{listed}");
    }

    #[test]
    fn verbs_for_the_running_app_say_when_it_isnt() {
        let run = |s: &str| cli(s).1;
        let e = run("call no-such-app-0a1b notes set --as ops x").unwrap_err();
        assert_eq!(e, "no-such-app-0a1b isn't running (nothing was sent)");
        assert!(run("clients no-such-app-0a1b notes").unwrap_err().contains("isn't running"));
        assert!(run("state no-such-app-0a1b notes --watch").unwrap_err().contains("isn't running"));
        // `--as` and `--args` take the next word: it is no positional.
        assert!(run("call no-such-app-0a1b notes set --args [1]").unwrap_err().contains("isn't running"));
        assert!(run("call no-such-app-0a1b notes set 1 --args [1]").unwrap_err().contains("not both"));
        assert!(run("dispatch notes:set x").unwrap_err().contains("ax call"));
        assert_eq!(cli("call no-such-app-0a1b notes set").0, 1);
    }
}

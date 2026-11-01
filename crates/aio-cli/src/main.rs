//! `ax`: aio-rs's command (aio: `am`). Read-only views into an app's store work
//! while the app runs (SQLite WAL readers never block its writer).
mod agent;
/// APKs: made by the NDK and Gradle (Linux, macOS, Windows).
mod android;
mod auth;
mod build;
mod bundle;
mod cdp;
mod components;
mod create;
mod css;
mod dev;
mod display;
mod doctor;
mod eval;
mod fleet;
mod freeze;
mod headless;
mod icon;
/// `ax install --android`: next to the APK build.
mod install;
mod installed;
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
mod places;
mod platform;
mod preview;
mod prune;
mod record;
mod release;
mod remote;
mod service;
mod setup;
mod shot;
mod sizes;
mod smoke;
mod sql;
mod start;
mod status;
mod stop;
mod surface;
mod target;
mod testrun;
mod theme;
mod travel;
mod trust;
mod ui;
mod verbs;
mod version;
mod video;
mod web;

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
/// first), each under the start of the app it came from; the failed build
/// `build` (`ax dev`'s `build.log`) first.
fn errors(build: Option<&str>, lines: &[&str], n: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    if let Some(b) = build {
        out.push(format!("{} (the app runs its last good build)", b.lines().next().unwrap_or("build failed")));
        out.extend(b.lines().skip(1).take(60).map(|l| format!("  {l}")));
    }
    // (start, its line, the bad lines after it)
    let mut runs: Vec<(usize, String, Vec<&str>)> = vec![(0, String::new(), vec![])];
    for l in lines {
        let rest = l.split_once(' ').map_or("", |(_, r)| r);
        if rest.starts_with("[aio] ") && rest.contains(" starting (pid ") {
            runs.push((runs.len(), l.to_string(), vec![]));
        } else if (rest.starts_with("WARN ") || rest.starts_with("ERROR ") || rest.contains(" panicked at "))
            && let Some(r) = runs.last_mut()
        {
            r.2.push(l);
        }
    }
    let total: usize = runs.iter().map(|r| r.2.len()).sum();
    let mut skip = total.saturating_sub(n);
    for (k, start, bad) in runs {
        let take = bad.len().saturating_sub(skip);
        skip = skip.saturating_sub(bad.len());
        if take == 0 {
            continue;
        }
        out.push(if k == 0 { "before the first start in the log:".into() } else { format!("start {k}: {start}") });
        out.extend(bad[bad.len() - take..].iter().map(|l| format!("  {l}")));
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
    let v: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(|e| e.to_string())?;
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
            let args = match args.get(aio::REDACTED).is_some() {
                true => serde_json::Value::String("[redacted]".into()),
                false => args,
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
        aio_shell::runtime::prefetch(VERSION, pinned_sha(VERSION).as_deref()).map_err(|e| e.to_string())?;
    }
    aio_shell::runtime::ensure("aio-rs")
        .map(|d| format!("Electron {} at {}", aio_shell::runtime::VERSION, d.display()))
        .map_err(|e| e.to_string())
}

/// The binary `ax start` runs for `t`: built now in its workspace, else
/// the installed one (its bin link).
fn app_bin(t: &Target) -> Result<PathBuf, String> {
    match &t.pkg {
        Some(pkg) => dev::build(&pkg.name, Some(&t.plan.logs))
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

/// `data/health.json`, judged now.
fn health(data: &Path) -> Out {
    let file = data.join("health.json");
    let text = std::fs::read(&file).map_err(|_| format!("not running (no {})", file.display()))?;
    let doc: aio::serde_json::Value = aio::serde_json::from_slice(&text).map_err(|e| e.to_string())?;
    let alive = aio::paths::running(data);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    aio::health::judge(&doc, now, alive)
}

/// The problem reports: a list, or the one whose id starts with `id`.
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

fn pin(t: &Target, plain: bool) -> Out {
    let pin = aio::rt().block_on(aio::pairing::mint(&t.plan.data, plain)).map_err(|e| e.to_string())?;
    Ok(format!("{pin}\n(valid 3 minutes, once: enter it on the new device)"))
}

/// Where a backup of the store at `store` goes unless named: `backups/`
/// in the app's home, beside its data (aio's place: never the cwd, which is
/// the app's checkout, where git would offer to commit it).
fn backup_file(app: &str, store: &Path, ms: i64) -> PathBuf {
    let home = store.parent().and_then(Path::parent).unwrap_or(Path::new("."));
    home.join("backups").join(format!("{app}-{}.aio-backup", aio::log::stamp(ms).replace([':', '.'], "-")))
}

fn backup(t: &Target, file: Option<&str>) -> Out {
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
    let (app, data) = (&t.id, &t.plan.data);
    let profile = aio::rt().block_on(aio::pairing::pair(addr, code)).map_err(|e| e.to_string())?;
    if profile.name != *app {
        return Err(format!("{addr} serves {}, not {app}", profile.name));
    }
    aio::paths::private_dir(data).map_err(|e| e.to_string())?;
    aio::pairing::save(data, addr, &profile).map_err(|e| e.to_string())?;
    Ok(format!(
        "paired {app} with {addr}{}",
        if profile.tls { " (TLS, root pinned)" } else { " (no TLS: use it only on a network you trust)" }
    ))
}

/// `ax profile`: the profile, printed or written 0600 (it holds the key).
fn profile(t: &Target, out: Option<&str>) -> Out {
    let (p, local) = aio::pairing::local_profile(&t.plan.data, &t.id).map_err(|e| e.to_string())?;
    if local {
        eprintln!(
            "[aio] {} listens on this machine only, so this profile connects from here only: \
             listen on 0.0.0.0 to hand it to another device",
            t.name
        );
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

/// Run a verb as `verbs::read` gave it: the verb, its flags, `--`, its words.
fn run(args: &[String]) -> Out {
    let cut = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (flags, pos) = (&args[1..cut], args.get(cut + 1..).unwrap_or_default());
    let has = |f: &str| flags.iter().any(|a| a == f);
    let json = has("--json");
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
        ("dev", [name, "--web"]) => web::dev(name),
        ("dev", [name, ..]) => dev::run(name, &pos[1..]),
        ("test", [name]) if has("--android") => install::lane(name, value("--device")),
        ("test", [name, filter @ ..]) if filter.len() <= 1 => {
            testrun::run(name, filter.first().copied(), value("--video"), value("--pace"))
        }
        ("preview", [app, component, props @ ..]) if props.len() <= 1 => {
            let (lib, dir) = app_lib(app)?;
            preview::run(&lib, &dir, component, props.first().copied(), has("--html"))
        }
        ("create", [name]) => {
            let aio = match (value("--aio"), create::this_checkout()) {
                (Some(p), _) => PathBuf::from(p),
                (None, Some(p)) => p.to_path_buf(),
                (None, None) => {
                    return Err("this ax was released without its checkout: name one with --aio <dir>".into());
                }
            };
            let here = std::env::current_dir().map_err(|e| e.to_string())?;
            let (target, installed) = (dev::shared_target(), places().install);
            let template = value("--template").unwrap_or("app");
            let made =
                create::run(&here, (name, value("--id")), template, &aio, (&target, &installed), has("--force"))?;
            Ok(made + &create::lock(&here.join(name)))
        }
        ("keygen", []) if has("--aio") => release::keygen_aio(&release::keys_dir(), has("--rotate")),
        ("keygen", [name]) => release::keygen(&release::keys_dir(), &app_id(name)?, has("--rotate")),
        ("release", []) if has("--aio") => {
            let targets: &[&str] = if has("--windows") { &["x86_64-pc-windows-gnu"] } else { &[] };
            release::aio(targets, value("--dir").map(Path::new), value("--url").unwrap_or(""))
        }
        ("release", [name]) if has("--workflow") => release::write_workflow(name, value("--channel")),
        ("release", [name]) => {
            let list = |f| value(f).map(|v| v.split(',').filter(|v| !v.is_empty()).collect()).unwrap_or_default();
            let mut platforms: Vec<&str> = list("--platforms");
            platforms.extend(has("--windows").then_some("windows"));
            let ask = release::Ask {
                channel: value("--channel"),
                notes: value("--notes").unwrap_or(""),
                platforms: platform::resolve(&platforms, platform::host()?)?,
                patch_from: list("--patch-from"),
                dir: value("--dir").map(Path::new),
                data: value("--data").map(Path::new),
                no_build: has("--no-build"),
                allow_dirty: has("--allow-dirty"),
                appimage: has("--appimage"),
                unsigned: has("--unsigned"),
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
        ("lab", [app, os]) => lab::run(app, os, has("--stop")),
        ("smoke", [artifact]) => {
            let file = Path::new(artifact);
            let id =
                smoke::id_of(file).ok_or_else(|| format!("{artifact}: not <id>-<version>-… as ax build names it"))?;
            smoke::run(file, id, smoke::Wait::Window)
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
        ("build", [app]) => {
            // The kind flags name targets too; --deb and --appimage come with the bundle.
            let mut select: Vec<&str> = value("--targets").into_iter().collect();
            select.extend(["--android", "--windows"].iter().filter(|f| has(f)).map(|f| &f[2..]));
            if has("--deb") || has("--appimage") {
                select.push("desktop");
            }
            select.extend(["--deb", "--appimage"].iter().filter(|f| has(f)).map(|f| &f[2..]));
            let select = select.join(",");
            build::run(app, Some(select.as_str()).filter(|s| !s.is_empty()), value("--platforms"), has("--smoke"))
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
            let found = aio::rt().block_on(aio::discovery::probe_lan(aio::discovery::port()));
            found.map(|f| places::discovered(&f, json)).map_err(|e| format!("can't probe the LAN: {e}"))
        }
        ("feedback", name) => places::feedback(&places::feedback_dir(&env), name.first().copied(), has("--create")),
        ("events", [app, cell @ ..]) => {
            let (cell, n) = (cell.first().copied(), last.unwrap_or(20));
            let t = t(app)?;
            // `--from`: another store (a backup, a copy) read as the journal.
            let db = open(&value("--from").map_or_else(|| t.store(), PathBuf::from))?;
            if !has("--diff") {
                return events(&db, cell, n, json, &|_| vec![]);
            }
            let cell = cell.ok_or("--diff shows one cell's changes: ax events <app> <cell> --diff")?;
            let seqs: Vec<u64> = travel::log(&db, cell)?.iter().rev().take(n as usize).map(|(s, _)| *s).collect();
            let at = travel::states(&door(&t, cell)?, cell, &user(None), &seqs)?;
            let none = serde_json::Value::Null;
            let diff = |seq: i64| {
                let (b, a) = (at.get(&(seq as u64).saturating_sub(1)), at.get(&(seq as u64)));
                travel::diff(b.unwrap_or(&none), a.unwrap_or(&none), "")
            };
            events(&db, Some(cell), n, json, &diff)
        }
        ("errors", [app]) => {
            let logs = t(app)?.plan.logs;
            let build = std::fs::read_to_string(logs.join(dev::BUILD_LOG)).ok();
            let text = log_lines(&logs, 100_000).unwrap_or_default();
            Ok(errors(build.as_deref(), &text.lines().collect::<Vec<_>>(), last.unwrap_or(20) as usize))
        }
        ("logs", [app, word @ ..]) => {
            let f: Vec<&str> = flags.iter().map(String::as_str).collect();
            let mut f = logs::filter(&f, now_ms())?;
            f.text = word.first().map(|w| w.to_string());
            let (dir, n) = (t(app)?.plan.logs, last.unwrap_or(50) as usize);
            if has("--follow") { logs::watch(&dir, n, &f) } else { logs::tail(&dir, n, &f, json) }
        }
        ("health", [app]) => health(&t(app)?.plan.data),
        ("stop", []) => stop::all(wait()?),
        ("stop", apps) => {
            let wait = wait()?;
            let all: Vec<Out> = apps.iter().map(|app| stop::one(app, &t(app)?, false, wait)).collect();
            let text = all.iter().map(|r| r.as_ref().unwrap_or_else(|e| e).as_str()).collect::<Vec<_>>().join("\n");
            if all.iter().all(Result::is_ok) { Ok(text) } else { Err(text) }
        }
        ("kill", [app]) => stop::one(app, &t(app)?, true, std::time::Duration::ZERO),
        ("start" | "restart", []) => {
            let wait = match value("--wait") {
                Some(_) => wait()?,
                None => start::WAIT,
            };
            let typed: Vec<String> = has("--cdp").then(|| "--cdp".to_string()).into_iter().collect();
            let one = |n: &str| {
                let t = t(n)?;
                match args[0].as_str() {
                    "restart" => start::restart(n, &t, &|| app_bin(&t), &typed, wait),
                    _ => start::run(&t, &app_bin(&t)?, &typed, value("--display"), wait),
                }
            };
            start::each(&stop::workspace_apps()?, &one)
        }
        ("start" | "restart", [app, words @ ..]) => {
            let t = t(app)?;
            let wait = match value("--wait") {
                Some(_) => wait()?,
                None => start::WAIT,
            };
            let mut typed = start::home_args(&ask);
            typed.extend(words.iter().map(|w| w.to_string()));
            typed.extend(has("--cdp").then(|| "--cdp".to_string()));
            if args[0] == "restart" {
                return start::restart(app, &t, &|| app_bin(&t), &typed, wait);
            }
            if has("--fresh") {
                dev::throw_away(Some(&t.plan))?;
            }
            start::run(&t, &app_bin(&t)?, &typed, value("--display"), wait)
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
        ("report", [app]) => report(&t(app)?.plan.data, None, json),
        ("report", [app, id]) => report(&t(app)?.plan.data, Some(id), json),
        ("state", [app, cell, path @ ..]) if has("--watch") => {
            live::watch(&door(&t(app)?, cell)?, cell, path.first().copied(), &user(value("--as")))
        }
        // Under --json, the state alone (the path "" is all of it).
        ("state", [app, cell]) => state(&t(app)?, cell, json.then_some(""), value("--as")),
        ("state", [app, cell, path]) => state(&t(app)?, cell, Some(path), value("--as")),
        ("call", [app, cell, method, words @ ..]) => {
            let args = live::call_args(words, value("--args"))?;
            live::call(&door(&t(app)?, cell)?, cell, method, args, &user(value("--as")))
        }
        ("expect", [app, cell, path, op, want @ ..]) => {
            let wait = match value("--wait") {
                Some(s) => std::time::Duration::from_secs_f64(s.parse().map_err(|_| "--wait needs seconds")?),
                None => std::time::Duration::ZERO,
            };
            live::expect_usage(op, want.first().copied())?;
            live::expect(&door(&t(app)?, cell)?, cell, (path, op, want.first().copied()), wait, &user(value("--as")))
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
            video::run(Path::new(value("--video").unwrap_or_default()), &at, until, journal)
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
            shot::run(&out, (&at, window()?), (value("--selector"), has("--full"), size), base, tol)
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
                    false => surface::read(&mut s, path.first().copied(), has("--rects")).await,
                }
            })
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
                let mut s = cdp::window_within(&at, window()?, wait_client()?).await?;
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
        ("heap", [app]) => {
            let t = t(app)?;
            let data = &t.plan.data;
            let now = || {
                let health: Option<serde_json::Value> = aio::paths::running(data)
                    .then(|| std::fs::read(data.join("health.json")).ok())
                    .flatten()
                    .and_then(|b| serde_json::from_slice(&b).ok());
                heap(&open(&t.store())?, health.as_ref(), json)
            };
            if !has("--watch") {
                return now();
            }
            // A top view: again every 2 s, until stopped.
            loop {
                let at =
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
                println!("{}\n{}\n", aio::log::stamp(at as i64), now()?);
                std::thread::sleep(std::time::Duration::from_secs(2));
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
            travel::load(&|cell| door(&t, cell), &text, &user(None))
        }
        ("peer", [app]) => {
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
            let cells: Vec<serde_json::Value> = metrics::cells(&open(&t.store())?)?.iter().filter_map(one).collect();
            if cells.is_empty() && !aio::paths::running(&t.plan.data) {
                return Err(format!("{app} isn't running: ax peer asks the running app"));
            }
            Ok(live::peer(&cells, json))
        }
        ("cost", [app, cell @ ..]) => {
            let t = t(app)?;
            let cells = match cell.first() {
                Some(c) => vec![c.to_string()],
                None => metrics::cells(&open(&t.store())?)?,
            };
            let mut rows = vec![];
            for c in cells {
                let link = door(&t, &c)?;
                match travel::ask(&link, &c, &user(None), serde_json::json!({ "t": "cost" })) {
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
        ("record", [app, cell, file, spec @ ..]) => {
            let t = t(app)?;
            let db = open(&value("--from").map_or_else(|| t.store(), PathBuf::from))?;
            let rows: Vec<travel::Row> = travel::rows(&db, travel::range(spec.first().copied())?)?
                .into_iter()
                .filter(|r| r.cell == *cell)
                .collect();
            let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
                return Err(format!("no events of {cell} in that range (ax events {app} {cell} lists them)"));
            };
            let at = travel::states(&door(&t, cell)?, cell, &user(None), &[first.seq, last.seq])?;
            let none = serde_json::Value::Null;
            let (before, after) = (at.get(&(first.seq - 1)).unwrap_or(&none), at.get(&last.seq).unwrap_or(&none));
            let krate = t.pkg.as_ref().map_or(t.id.clone(), |p| p.name.replace('-', "_"));
            let ty = value("--type").map_or_else(|| format!("{krate}::{}", record::type_of(cell)), String::from);
            let text = record::test(app, &ty, &record::name_of(file), before, &rows, after, record::exact(&rows));
            if Path::new(file).exists() {
                return Err(format!("{file} exists: pick a new name"));
            }
            std::fs::write(file, text).map_err(|e| format!("{file}: {e}"))?;
            Ok(format!("wrote {file}: {} events of {cell}; its `use {ty};` must name the cell's type", rows.len()))
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
            prune::run(&aio_shell::runtime::root(), &f, std::time::SystemTime::now())
        }
        ("runtime", []) => runtime(has("--fetch")),
        ("setup", []) => setup::run(has("--check")),
        ("pin", [app]) => pin(&t(app)?, has("--plain")),
        ("pair", [app, addr, code]) => pair(&t(app)?, addr, code),
        ("auth", [app, what @ ..]) => {
            let f = auth::Fields { role: value("--role"), email: value("--email"), password: value("--password") };
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
            auth::run(&t(app)?.plan.data, what, &f, json, now as i64)
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
            let open = toml
                .as_ref()
                .and_then(|f| std::fs::read_to_string(f).ok())
                .and_then(|s| aio::config::Config::parse(&s).ok())
                .filter(|c| c.server.open)
                .map(|c| c.app.id);
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
            };
            Ok(doctor::render(&facts))
        }
        ("profile", [app]) => profile(&t(app)?, value("--out")),
        ("import", [app, file]) => {
            let t = t(app)?;
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            let addr = aio::pairing::import(&t.plan.data, &t.id, &text).map_err(|e| e.to_string())?;
            Ok(format!("linked {} to {addr}", t.name))
        }
        ("backup", [app]) => backup(&t(app)?, None),
        ("backup", [app, file]) => backup(&t(app)?, Some(file)),
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
        Err(e) => serde_json::json!({ "ok": false, "error": e, "exit": code }),
    };
    v.to_string()
}

/// Run beside an app's folder (`ax dev notes` where `notes/` is, in no
/// workspace): the app named that folder, as if after `cd notes` (pure:
/// which folder, if any). Only the verbs on an app's source; `in_workspace`:
/// a Cargo.toml here or above, so nothing to step into.
fn steps_in<'a>(args: &'a [String], in_workspace: bool, has_manifest: &dyn Fn(&str) -> bool) -> Option<&'a str> {
    const SOURCE: [&str; 9] = ["dev", "test", "build", "release", "keygen", "preview", "config", "lab", "install"];
    let (verb, app) = (args.first()?, args.get(1)?);
    let plain = !app.starts_with('-') && !app.contains(['/', '\\', '.']);
    (SOURCE.contains(&verb.as_str()) && plain && !in_workspace && has_manifest(app)).then_some(app.as_str())
}

fn main() -> ExitCode {
    // Every cargo and dx ax runs builds into one folder per machine.
    // SAFETY: before any thread is started.
    unsafe { std::env::set_var("CARGO_TARGET_DIR", dev::shared_target()) };
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Its own exit codes (aio's aiol): 1 errors found, 2 bad usage.
    if args.first().map(String::as_str) == Some("lint") {
        return lint::main(&args[1..]);
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
    match (&out, json) {
        (_, true) => println!("{}", as_json(code, &out)),
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
        let all = errors(None, &log, 20);
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
        // The newest n, still under their start.
        assert_eq!(errors(None, &log, 1), "start 2: t3 [aio] notes 2.0.0 starting (pid 11)\n  t5 WARN  [x] two");
        assert!(errors(None, &log[..1], 20).starts_with("no warnings or errors in the log (1 lines)"));
        let b = dev::build_log(0, "error[E0425]: cannot find value `x`\n --> src/lib.rs:1:1");
        let out = errors(Some(&b), &[], 20);
        assert!(out.starts_with(
            "build failed at 1970-01-01T00:00:00.000Z (the app runs its last good build)\n  error[E0425]"
        ));
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
        assert!(cli("").1.unwrap().contains("ax apps"));
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

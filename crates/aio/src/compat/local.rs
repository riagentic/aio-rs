//! aio's view of a running aio-rs app: its lock record in aio's lock folder
//! (so `am instances` lists it) and aio's local socket, NDJSON lines of
//! aio's envelope (so `am state` and `am dispatch` reach it without `/ws`).
//!
//! The record is `<lock dir>/<key>.lock` as aio writes it (`appId`, `pid`,
//! `port`, `socketPath`, `home`, `startToken`…), removed at a clean stop;
//! one a crash leaves is aio's to judge dead and remove, by its pid and
//! start stamp. The socket answers aio's control plane (`ctl` → `ctlr`):
//! `/__aio/health`, and on a debug build (aio's trojan is dev-only)
//! `/__aio/trojan/state`, `client-cells` and `dispatch`, each behind the
//! `X-Aio-Control` key aio's `am` reads from `<home>/data/control.key`.
//! Every call goes through the app's own doors ([`crate::door`]), as
//! `ax call` does: the same gates, the same log.
use crate::door::{self, Frames};
use crate::link::Msg;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

/// The cells this process serves, each with its door.
static CELLS: Mutex<Vec<(&'static str, String)>> = Mutex::new(Vec::new());
/// This run as aio sees it: its control key and lock record ([`Me`]).
static ME: Mutex<Option<Me>> = Mutex::new(None);
static STARTED: OnceLock<()> = OnceLock::new();

/// aio's `hash8`: FNV-1a over UTF-16 code units, as 8 hex digits (pure).
pub(crate) fn hash8(s: &str) -> String {
    let h = s.encode_utf16().fold(0x811c_9dc5u32, |h, c| (h ^ u32::from(c)).wrapping_mul(0x0100_0193));
    format!("{h:08x}")
}

/// aio's lock folder's scope for `AIO_APPS_DIR` (`apps`: that root made
/// absolute): `-` and its last 48 letters and digits, runs of others as
/// one `-` (pure); none: empty.
pub(crate) fn scope(apps: Option<&Path>) -> String {
    let Some(root) = apps else { return String::new() };
    let mut out = String::new();
    for c in root.to_string_lossy().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    let tail: String = out.chars().skip(out.chars().count().saturating_sub(48)).collect();
    format!("-{tail}")
}

/// aio's profile name (`profileNameError` is none): 1-32 of `a-z0-9-`,
/// not starting with `-`, not `default`, not 8 hex digits (pure).
fn profile_name(p: &str) -> bool {
    let b = p.as_bytes();
    (1..=32).contains(&b.len())
        && b[0] != b'-'
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && p != "default"
        && !(b.len() == 8 && b.iter().all(u8::is_ascii_hexdigit))
}

/// The record's key (pure, aio's `lockKey`): the app id for aio's default
/// home (`<user home>/.<id>`, or `<AIO_APPS_DIR>/<id>`); `<id>@<profile>`
/// when the home is a profile's (it ends in `-<profile>`, or it is the
/// default home's `-<name>`); else `<id>@<hash8 of home>`.
pub(crate) fn key(id: &str, home: &Path, profile: Option<&str>, user: &Path, apps: Option<&Path>) -> String {
    let default = apps.map_or_else(|| user.join(format!(".{id}")), |a| a.join(id));
    if home == default {
        return id.to_string();
    }
    let h = home.to_string_lossy();
    let named = profile.filter(|p| h.ends_with(&format!("-{p}")) && profile_name(p)).or_else(|| {
        // Beside the default home, named `<its name>-<profile>` (by path
        // parts, so either separator).
        let base = default.file_name()?.to_str()?;
        let p = home.file_name()?.to_str()?.strip_prefix(base)?.strip_prefix('-')?;
        (home.parent() == default.parent() && profile_name(p)).then_some(p)
    });
    match named {
        Some(p) => format!("{id}@{p}"),
        None => format!("{id}@{}", hash8(&h)),
    }
}

/// aio's lock folder: `<XDG_RUNTIME_DIR or /tmp>/aio<scope>` (Windows:
/// `%TEMP%`), made 0700 and this user's; else `aio-u<uid><scope>`.
pub(crate) fn lock_dir() -> std::io::Result<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let apps = env("AIO_APPS_DIR").and_then(|a| resolve(&a).ok());
    let scope = scope(apps.as_deref());
    #[cfg(unix)]
    {
        let base = env("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into());
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let first = base.join(format!("aio{scope}"));
        if private(&first, uid).is_ok() {
            return Ok(first);
        }
        let other = base.join(format!("aio-u{uid}{scope}"));
        private(&other, uid)?;
        Ok(other)
    }
    #[cfg(windows)]
    {
        let base = env("TEMP").or_else(|| env("TMP")).unwrap_or_else(|| r"C:\Temp".into());
        let dir = base.join(format!("aio{scope}"));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
}

/// `d` a folder of `uid`'s that only it may open (made so if it is new or
/// merely too open), as aio's `judgeLockDir` takes one.
#[cfg(unix)]
fn private(d: &Path, uid: u32) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let made = std::fs::DirBuilder::new().mode(0o700).create(d);
    if let Err(e) = made
        && e.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(e);
    }
    let m = std::fs::symlink_metadata(d)?;
    if !m.is_dir() || m.uid() != uid {
        return Err(std::io::Error::other(format!("{}: not a folder of this user's", d.display())));
    }
    if m.mode() & 0o077 != 0 {
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Where aio's local socket for `key` is in `dir` (aio's `socketPath`):
/// `<dir>/<key>.sock`, a hashed name where that is past 100 bytes;
/// Windows: the named pipe `\\.\pipe\aio-<key>` (`aio+<scope>+<key>` scoped).
pub(crate) fn socket(dir: &Path, key: &str, id: &str) -> String {
    if cfg!(windows) {
        let d = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return match d.strip_prefix("aio-").or_else(|| d.strip_prefix("aio")).filter(|s| !s.is_empty()) {
            Some(scope) => format!(r"\\.\pipe\aio+{scope}+{key}"),
            None => format!(r"\\.\pipe\aio-{key}"),
        };
    }
    let p = dir.join(format!("{key}.sock")).to_string_lossy().into_owned();
    match p.len() > 100 {
        true => {
            let short: String = id.chars().take(40).collect();
            dir.join(format!("{short}-{}.sock", hash8(&p))).to_string_lossy().into_owned()
        }
        false => p,
    }
}

/// aio's start stamp of process `pid`: Linux, `/proc/<pid>/stat`'s 22nd
/// field; `None` elsewhere.
pub(crate) fn start_token(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = stat.get(stat.rfind(')')? + 2..)?;
    rest.split(' ').nth(19).filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())).map(String::from)
}

/// The record aio reads (pure, but for the pid and clock passed in). No
/// socket: listed by `am instances`, reached by nothing (an app that only
/// calls aio apps, [`super::AioPeer`]).
pub(crate) fn record(
    id: &str,
    home: &Path,
    socket: Option<&str>,
    profile: Option<&str>,
    (pid, at): (u32, i64),
) -> Value {
    let mut r = json!({
        "appId": id,
        "pid": pid,
        "port": 0,
        "startedAt": at,
        "status": "started",
        "cwd": std::env::current_dir().unwrap_or_default(),
        "home": home,
        "dataDir": home.join("data"),
        "aioVersion": format!("aio-rs {}", env!("CARGO_PKG_VERSION")),
    });
    if let Some(s) = socket {
        r["socketPath"] = s.into();
    }
    if let Some(p) = profile {
        r["profile"] = p.into();
    }
    if let Some(t) = start_token(pid) {
        r["startToken"] = t.into();
    }
    r
}

/// Serve cell `cell`, whose door is `addr`, to aio's tools too; the first
/// one opens aio's socket for app `id` (data folder `data`, `profile`) and
/// writes its lock record. Said, never fatal: aio's view is extra.
pub fn serve(id: &str, data: &Path, profile: Option<&str>, cell: &'static str, addr: String) {
    lock(&CELLS).push((cell, addr));
    if STARTED.set(()).is_err() {
        return;
    }
    if let Err(e) = open(id, data, profile) {
        eprintln!("[aio] aio's tools can't see this app (am instances, am state): {e}");
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `p` absolute, `.` and `..` folded away (Node's `path.resolve`, as aio
/// compares homes).
pub(crate) fn resolve(p: &Path) -> std::io::Result<PathBuf> {
    use std::path::Component;
    Ok(std::path::absolute(p)?.components().fold(PathBuf::new(), |mut acc, c| {
        match c {
            Component::ParentDir => _ = acc.pop(),
            Component::CurDir => {}
            c => acc.push(c),
        }
        acc
    }))
}

/// This run as aio sees it: its control key (one per run: `am`'s
/// `X-Aio-Control` and the `x-aio-peer` proofs both use it) and its lock
/// record, once written.
struct Me {
    id: String,
    data: PathBuf,
    profile: Option<String>,
    control: String,
    record: Option<(PathBuf, String)>,
}

/// This run's control key, made and written to `<data>/control.key` (0600)
/// on the first ask.
fn me<'a>(slot: &'a mut Option<Me>, id: &str, data: &Path, profile: Option<&str>) -> std::io::Result<&'a mut Me> {
    if slot.is_none() {
        let control = hex(&crate::random_bytes::<32>());
        crate::paths::write_atomic(&data.join("control.key"), format!("{control}\n").as_bytes(), true)?;
        *slot =
            Some(Me { id: id.into(), data: data.into(), profile: profile.map(String::from), control, record: None });
    }
    Ok(slot.as_mut().expect("set above"))
}

/// Write `me`'s record (with its socket, if it has one) in aio's lock folder.
fn write_record(me: &mut Me, socket: Option<&str>) -> std::io::Result<PathBuf> {
    let (home, k, dir) = place(&me.id, &me.data, me.profile.as_deref())?;
    let text = record(&me.id, &home, socket, me.profile.as_deref(), (std::process::id(), now_ms())).to_string();
    let file = dir.join(format!("{k}.lock"));
    crate::paths::write_atomic(&file, text.as_bytes(), true)?;
    me.record = Some((file.clone(), text));
    Ok(file)
}

/// Where app `id` (data folder `data`) is filed: its home, its key, aio's
/// lock folder.
fn place(id: &str, data: &Path, profile: Option<&str>) -> std::io::Result<(PathBuf, String, PathBuf)> {
    let home = resolve(data.parent().unwrap_or(data))?;
    let user =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
    let apps =
        std::env::var_os("AIO_APPS_DIR").filter(|v| !v.is_empty()).map(|a| resolve(Path::new(&a))).transpose()?;
    let k = key(id, &home, profile, &user, apps.as_deref());
    Ok((home, k, lock_dir()?))
}

/// The socket, the control key, the record: in that order, so a record
/// always names a socket that answers.
fn open(id: &str, data: &Path, profile: Option<&str>) -> std::io::Result<PathBuf> {
    let (_, k, dir) = place(id, data, profile)?;
    let at = socket(&dir, &k, id);
    // A socket file a crashed run left (its record is aio's to drop).
    if cfg!(unix) {
        _ = std::fs::remove_file(&at);
    }
    let _in = crate::rt().enter();
    let l = crate::ipc::Listener::bind(&at)?;
    let mut slot = lock(&ME);
    let me = me(&mut slot, id, data, profile)?;
    crate::rt().spawn(accept(l, me.control.clone()));
    write_record(me, Some(&at))
}

/// This app as an aio app on this machine: its id and control key, its
/// lock record written (without a socket, if it serves none) so an aio app
/// can check a proof made with the key ([`super::AioPeer`]).
#[cfg(feature = "server")]
pub(crate) fn identity() -> std::io::Result<(String, String)> {
    let dirs = crate::paths::app_dirs()?;
    let mut slot = lock(&ME);
    let me = me(&mut slot, &dirs.id, &dirs.data, dirs.profile.as_deref())?;
    if me.record.is_none() {
        write_record(me, None)?;
    }
    Ok((me.id.clone(), me.control.clone()))
}

/// At a clean stop: the record and the control key removed, each if it is
/// still this run's (aio's shutdown removes both).
pub(crate) fn remove_record() {
    let Some(me) = lock(&ME).take() else { return };
    if let Some((file, text)) = me.record
        && std::fs::read_to_string(&file).is_ok_and(|t| t == text)
    {
        _ = std::fs::remove_file(file);
    }
    let key = me.data.join("control.key");
    if std::fs::read_to_string(&key).is_ok_and(|k| k.trim() == me.control) {
        _ = std::fs::remove_file(key);
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

async fn accept(mut l: crate::ipc::Listener, control: String) {
    loop {
        let Ok(s) = l.accept().await else {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            continue;
        };
        let control = control.clone();
        tokio::spawn(async move {
            if let Err(e) = lines(s, &control).await {
                eprintln!("[aio] aio's socket: {e}");
            }
        });
    }
}

/// aio's longest line on its socket.
const MAX_LINE: usize = 10 << 20;

/// One aio client: each `ctl` line answered by a `ctlr` line; `type`,
/// `proto` and pings needn't any.
async fn lines(s: crate::ipc::Stream, control: &str) -> std::io::Result<()> {
    let (r, mut w) = tokio::io::split(s);
    let mut r = tokio::io::BufReader::new(r);
    let mut line = Vec::new();
    loop {
        line.clear();
        if (&mut r).take(MAX_LINE as u64).read_until(b'\n', &mut line).await? == 0 {
            return Ok(());
        }
        let Ok(m) = serde_json::from_slice::<Value>(&line) else { continue };
        if m["v"] != 2 || m["t"] != "ctl" {
            continue;
        }
        let d = &m["d"];
        let (status, body) = ctl(d, control).await;
        let reply = json!({ "v": 2, "t": "ctlr", "d": { "id": d["id"], "status": status, "body": body.to_string() } });
        w.write_all(format!("{reply}\n").as_bytes()).await?;
    }
}

/// One control-plane request (aio's trojan routes): its status and body.
async fn ctl(d: &Value, control: &str) -> (u16, Value) {
    let (method, path) = (d["method"].as_str().unwrap_or("GET"), d["path"].as_str().unwrap_or_default());
    let header = |k: &str| {
        d["headers"]
            .as_object()
            .and_then(|h| h.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)))
            .and_then(|(_, v)| v.as_str())
    };
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    if route == "/__aio/health" {
        return (200, health());
    }
    if !route.starts_with("/__aio/trojan/") {
        return (404, json!({ "error": format!("{route}: not served on aio's socket by an aio-rs app") }));
    }
    if !door::OPERATOR {
        return (404, json!({ "error": "trojan is disabled in production" }));
    }
    if !header("x-aio-control").is_some_and(|k| same(k.trim().as_bytes(), control.as_bytes())) {
        return (401, json!({ "error": "unauthorized: X-Aio-Control is <home>/data/control.key" }));
    }
    let cells = lock(&CELLS).clone();
    match (method, route) {
        ("GET", "/__aio/trojan/client-cells") => (200, json!(cells.iter().map(|(c, _)| *c).collect::<Vec<_>>())),
        ("GET", "/__aio/trojan/state") => {
            let mut all = serde_json::Map::new();
            for (cell, addr) in &cells {
                if let Ok((_, view)) = link(addr, cell, false).await {
                    all.insert(cell.to_string(), view);
                }
            }
            (200, Value::Object(all))
        }
        ("POST", "/__aio/trojan/dispatch") if header("x-aio").is_none() => {
            (403, json!({ "error": "Missing X-AIO header" }))
        }
        ("POST", "/__aio/trojan/dispatch") => {
            let body: Value = d["body"].as_str().and_then(|b| serde_json::from_str(b).ok()).unwrap_or_default();
            dispatch(&cells, &body, query.split('&').any(|q| q == "as=server")).await
        }
        _ => (404, json!({ "error": format!("{method} {route}: no such route on an aio-rs app") })),
    }
}

/// Equal, in time that doesn't depend on where they differ.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// `health.json` of the app, as `/__aio/health` answers.
fn health() -> Value {
    let about = crate::report::about();
    let doc = about.as_ref().and_then(|a| std::fs::read(a.dir.parent()?.join("health.json")).ok());
    doc.and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({ "status": "healthy", "pid": std::process::id() }))
}

/// aio's `{type: "cell:method", payload: {args}}` as a call on that cell's
/// door; a payload that is no `{args}` is the method's one argument.
async fn dispatch(cells: &[(&'static str, String)], body: &Value, server: bool) -> (u16, Value) {
    let ty = body["type"].as_str().unwrap_or_default();
    let Some((cell, method)) = ty.split_once([':', '.']) else {
        return (
            404,
            json!({ "error": format!("{ty}: aio-rs has no plain actions: a cell:method, or a server function (ax call --fn)") }),
        );
    };
    let Some((_, addr)) = cells.iter().find(|(c, _)| *c == cell) else {
        return (404, json!({ "error": format!("no cell {cell} in this app") }));
    };
    let p = &body["payload"];
    let args = match &p["args"] {
        Value::Array(a) => Value::Array(a.clone()),
        _ if p.is_null() => json!([]),
        _ => json!([p]),
    };
    match call(addr, cell, method, args, server).await {
        Ok(v) => (200, json!({ "ok": true, "result": v, "unsaved": null })),
        Err(e) => (409, json!({ "error": e })),
    }
}

/// A link to the cell on its door, as this user's tool: its frames and
/// the first view.
async fn link(addr: &str, cell: &str, server: bool) -> Result<(Frames, Value), String> {
    let mut f = door::dial(addr).await.map_err(|e| e.to_string())?;
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "am".into());
    let hello = json!({ "t": "hello", "cell": cell, "tool": true, "user": user, "peer": false, "server": server });
    door::send(&mut f, Msg::Text(hello.to_string())).await.map_err(|e| e.to_string())?;
    let first = next(&mut f).await?;
    match first["t"] == "state" {
        true => Ok((f, first["state"].clone())),
        false => Err(format!("refused: {}", first["why"].as_str().unwrap_or("?"))),
    }
}

/// The next JSON message on the door (its pings answered).
async fn next(f: &mut Frames) -> Result<Value, String> {
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(30), door::recv(f)).await {
            Ok(Some(Msg::Text(t))) => return serde_json::from_str(&t).map_err(|e| e.to_string()),
            Ok(Some(Msg::Ping(p))) => door::send(f, Msg::Pong(p)).await.map_err(|e| e.to_string())?,
            Ok(Some(Msg::Pong(_))) => {}
            Ok(Some(Msg::Close(_, why))) => return Err(why),
            Ok(None) => return Err("the app closed the link".into()),
            Err(_) => return Err("no answer in 30 s (it may still run)".into()),
        }
    }
}

/// `cell.method(args)` on its door: its answer (an async method's at its end).
async fn call(addr: &str, cell: &str, method: &str, args: Value, server: bool) -> Result<Value, String> {
    let (mut f, _) = link(addr, cell, server).await?;
    let msg = json!({ "t": "call", "method": method, "args": args });
    door::send(&mut f, Msg::Text(msg.to_string())).await.map_err(|e| e.to_string())?;
    let mut m = loop {
        let m = next(&mut f).await?;
        if m["t"] == "reply" {
            break m;
        }
    };
    if m["async"] == true {
        m = loop {
            let e = next(&mut f).await?;
            if e["t"] == "end" {
                break e;
            }
        };
    }
    match m["err"].as_str() {
        Some(e) => Err(e.to_string()),
        None => Ok(m["ok"].clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_hashes_and_scopes_are_aios() {
        // aio's hash8 of "" and "a" (FNV-1a 32).
        assert_eq!((hash8(""), hash8("a")), ("811c9dc5".into(), "e40c292c".into()));
        let (user, home) = (Path::new("/u"), Path::new("/u/.notes"));
        assert_eq!(key("notes", home, None, user, None), "notes");
        assert_eq!(key("notes", Path::new("/a/notes"), None, user, Some(Path::new("/a"))), "notes");
        assert_eq!(key("notes", Path::new("/u/.notes-work"), Some("work"), user, None), "notes@work");
        let other = Path::new("/u/.local/share/aio-rt/apps/notes");
        assert_eq!(key("notes", other, None, user, None), format!("notes@{}", hash8(&other.to_string_lossy())));
        assert_eq!(scope(None), "");
        assert_eq!(scope(Some(Path::new("/tmp/my apps//x"))), "-tmp-my-apps-x");
        assert_eq!(scope(Some(Path::new(&format!("/{}", "a".repeat(60))))).len(), 49);
        // aio's lockKey (server/lock-dir.ts) and hash8, run in deno: a profile
        // names the key only where the home is that profile's.
        assert_eq!(hash8("/home/u/.local/share/aio-rt/apps/notes"), "afede357");
        let (u, apps) = (Path::new("/home/u"), Path::new("/home/u/.local/share/aio-rt/apps/notes"));
        assert_eq!(key("notes", apps, Some("dev"), u, None), "notes@afede357");
        assert_eq!(key("notes", Path::new("/home/u/.notes-dev"), None, u, None), "notes@dev");
        assert_eq!(key("notes", Path::new("/opt/notes-dev"), Some("dev"), u, None), "notes@dev");
        let hex = Path::new("/home/u/.notes-0badf00d");
        assert_eq!(key("notes", hex, None, u, None), "notes@ad998c91");
        assert!(profile_name("dev") && profile_name("test-2") && !profile_name("Dev") && !profile_name("-x"));
        assert!(!profile_name("default") && !profile_name("0badf00d") && !profile_name(&"a".repeat(33)));
        let sock = record("notes", apps, None, None, (42, 7));
        assert!(sock.get("socketPath").is_none() && sock["port"] == 0, "a record no tool reaches: {sock}");
    }

    #[cfg(unix)]
    #[test]
    fn a_long_socket_path_is_hashed_short() {
        let dir = Path::new("/run/user/1000/aio");
        assert_eq!(socket(dir, "notes", "notes"), "/run/user/1000/aio/notes.sock");
        let long = "n".repeat(120);
        let s = socket(dir, &long, &long);
        assert!(s.len() < 100 && s.starts_with(&format!("/run/user/1000/aio/{}-", "n".repeat(40))), "{s}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_record_carries_what_aio_judges_it_by() {
        let r = record("notes", Path::new("/u/.notes"), Some("/r/aio/notes.sock"), Some("work"), (42, 7));
        assert_eq!((r["appId"].as_str(), r["pid"].as_u64(), r["port"].as_u64()), (Some("notes"), Some(42), Some(0)));
        assert_eq!(
            (r["status"].as_str(), r["socketPath"].as_str(), r["profile"].as_str()),
            (Some("started"), Some("/r/aio/notes.sock"), Some("work"))
        );
        assert!(r["startToken"].as_str().is_some_and(|t| t.parse::<u64>().is_ok()), "{r}");
    }
}

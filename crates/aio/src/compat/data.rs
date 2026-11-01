//! A ported app's aio data, as aio keeps it (docs/migration/data.md): its home
//! (`~/.<appId>`, profiles, `AIO_APPS_DIR`), the home claim, and the cell
//! snapshot in `state.db`'s `aio_kv`, read and written in aio's shapes so aio
//! can still open the file after aio-rs ran.
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value, json};
use std::io;
use std::path::{Path, PathBuf};

/// aio's multi-layout row separator: `<persistKey>\x1f<cell>`.
const SEP: char = '\x1f';
/// aio's `PERSIST_SCHEMA_VERSION`: a file stamped newer is refused.
const SCHEMA: i64 = 1;

/// aio's `profileNameError`: why `name` can't be a profile, or `None`.
fn profile_error(name: &str) -> Option<String> {
    let shaped = (1..=32).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-');
    if !shaped {
        return Some(format!(
            "profile {name:?} is not a profile name: use 1-32 lowercase letters, digits and dashes, starting with a letter or digit"
        ));
    }
    if name == "default" {
        return Some("profile \"default\" is reserved: the default is no profile".into());
    }
    // The shape of an isolated home's lock tag (`app@1a2b3c4d`).
    if name.len() == 8 && name.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(format!("profile {name:?} looks like a lock tag (8 hex digits): pick another name"));
    }
    None
}

/// aio's `isProfilePath`: a separator, a leading `~` or `.`, or a drive.
fn is_path(v: &str) -> bool {
    let b = v.as_bytes();
    v.contains(['/', '\\']) || v.starts_with(['~', '.']) || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// aio's `expandProfilePath`: `~` is the user's home, a relative path the cwd's.
pub(crate) fn expand(v: &str, user: &Path) -> PathBuf {
    let p = match v.strip_prefix("~/").or_else(|| v.strip_prefix("~\\")) {
        Some(rest) => user.join(rest),
        None if v == "~" => user.to_path_buf(),
        None => PathBuf::from(v),
    };
    std::path::absolute(&p).unwrap_or(p)
}

/// App `app`'s aio home and its profile, as aio resolves it (`app-dirs.ts`):
/// `home` (`--home`) > `profile` (`--profile`/`AIO_PROFILE`: a name gives
/// `<base>-<name>`, a path that folder) > `<apps_dir>/<app>` > `<user>/.<app>`.
/// A folder named `<base>-<valid name>` is that profile, however it was named.
pub fn resolve(
    app: &str,
    home: Option<&str>,
    profile: Option<&str>,
    apps_dir: Option<&Path>,
    user: &Path,
) -> Result<(PathBuf, Option<String>), String> {
    let base = apps_dir.map_or_else(|| user.join(format!(".{app}")), |r| r.join(app));
    pick(&base, home, profile, user)
}

/// [`resolve`] from `base`, the home an app has when nothing is asked.
pub(crate) fn pick(
    base: &Path,
    home: Option<&str>,
    profile: Option<&str>,
    user: &Path,
) -> Result<(PathBuf, Option<String>), String> {
    let one = |v: &str, flag: &str, path: bool| {
        if v.trim().is_empty() {
            return Err(format!("{flag} is empty: name a profile"));
        }
        if path || is_path(v) {
            return Ok(expand(v, user));
        }
        profile_error(v).map_or_else(|| Ok(PathBuf::from(format!("{}-{v}", base.display()))), Err)
    };
    let by_profile = profile.map(|v| one(v, "--profile/AIO_PROFILE", false)).transpose()?;
    let by_home = home.map(|v| one(v, "--home", true)).transpose()?;
    let chosen = match (by_home, by_profile) {
        (Some(h), Some(p)) if h != p => {
            return Err(format!(
                "--profile ({} → {}) and --home ({} → {}) name two different folders: give one of them",
                profile.unwrap_or_default(),
                p.display(),
                home.unwrap_or_default(),
                h.display()
            ));
        }
        (h, p) => h.or(p),
    };
    let Some(chosen) = chosen else { return Ok((base.to_path_buf(), None)) };
    let name = chosen
        .to_string_lossy()
        .strip_prefix(&format!("{}-", base.display()))
        .filter(|n| profile_error(n).is_none())
        .map(String::from);
    Ok((chosen, name))
}

/// aio's `RESERVED_APP_NAMES`: `~/.<name>` is another program's folder.
const RESERVED: [&str; 21] = [
    "aio",
    "ssh",
    "gnupg",
    "config",
    "local",
    "cache",
    "pki",
    "docker",
    "kube",
    "aws",
    "azure",
    "gcloud",
    "mozilla",
    "thunderbird",
    "deno",
    "npm",
    "cargo",
    "rustup",
    "git",
    "vscode",
    "password-store",
];

/// What aio puts directly in a home (aio's `AIO_HOME_ENTRIES`).
const OURS: [&str; 8] =
    ["data", "logs", "cache", "app", "backups", "launch.json", ".aio-instance.lock", ".aio-instance.json"];

/// What a desktop drops in any folder a person opens (aio's `OS_FOLDER_LITTER`).
const LITTER: [&str; 7] =
    [".ds_store", "._.ds_store", ".localized", "thumbs.db", "ehthumbs.db", "desktop.ini", ".directory"];

/// Why app `app` may not keep its data in `home`, or `None` (aio's
/// `foreignAppHomeError` + `reservedAppHomeError`): a file; a folder holding
/// files (desktop litter aside) none of which are aio's; or `<user>/.<a
/// reserved name>` that isn't an app's already (`data/` and `logs/`),
/// whether it exists yet or not.
pub fn refusal(app: &str, home: &Path, user: &Path) -> Option<String> {
    let fix = "give the app another id, or name its folder with --home=<dir>";
    if home.exists() && !home.is_dir() {
        return Some(format!("app {app:?} would keep its data in {}, which is a file: {fix}", home.display()));
    }
    let mut theirs: Vec<String> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !LITTER.contains(&n.to_lowercase().as_str()))
        .collect();
    if !theirs.is_empty() && !theirs.iter().any(|n| OURS.contains(&n.as_str())) {
        theirs.sort();
        let more = theirs.len().saturating_sub(3);
        theirs.truncate(3);
        let more = if more > 0 { format!(", …{more} more") } else { String::new() };
        return Some(format!(
            "app {app:?} would keep its data in {}, among another program's files ({}{more}): {fix}",
            home.display(),
            theirs.join(", ")
        ));
    }
    let an_app = home.join("data").is_dir() && home.join("logs").is_dir();
    if RESERVED.contains(&app.to_lowercase().as_str()) && home == user.join(format!(".{app}")) && !an_app {
        return Some(format!(
            "app {app:?} would keep its data in {}, a folder of another program's: {fix}",
            home.display()
        ));
    }
    None
}

/// [`resolve`] with aio's environment: `AIO_PROFILE` when no `profile` is
/// given, `AIO_APPS_DIR`, and `$HOME` (else `%USERPROFILE%`).
pub fn home(app: &str, home: Option<&str>, profile: Option<&str>) -> Result<(PathBuf, Option<String>), String> {
    let var = |k| std::env::var(k).ok().filter(|v| !v.is_empty());
    let env_profile = var("AIO_PROFILE");
    let apps = var("AIO_APPS_DIR").map(|r| expand(&r, &crate::paths::home()));
    resolve(app, home, profile.or(env_profile.as_deref()), apps.as_deref(), &crate::paths::home())
}

/// An aio home, claimed: `<home>/.aio-instance.lock` held (exclusive OS lock,
/// the one aio takes), so aio and aio-rs never run on the same data at once.
#[derive(Clone, Debug)]
pub struct Home {
    pub home: PathBuf,
    /// `<home>/data` (mode 0700): aio's backup unit.
    pub data: PathBuf,
    pub(crate) claim: std::sync::Arc<crate::paths::Held>,
}

fn refused(why: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}

impl Home {
    /// Claim `home` for app `app` (under `profile`). Refused, with nothing
    /// written, when another process holds it or its `meta.json` names
    /// another app or profile. (aio's journal is replayed by the store's boot.)
    pub fn open(home: &Path, app: &str, profile: Option<&str>) -> io::Result<Self> {
        let data = home.join("data");
        // The home holds logs and keys too: private, as aio makes it.
        crate::paths::private_dir(home)?;
        crate::paths::private_dir(&data)?;
        let mut o = std::fs::OpenOptions::new();
        o.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        let claim = o.open(home.join(".aio-instance.lock"))?;
        let info = home.join(".aio-instance.json");
        claim.try_lock().map_err(|e| match e {
            std::fs::TryLockError::WouldBlock => {
                let pid = std::fs::read(&info)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                    .and_then(|v| v["pid"].as_u64())
                    .map_or_else(String::new, |p| format!(" (pid {p})"));
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!("{} is in use by another app{pid}: stop it first", home.display()),
                )
            }
            std::fs::TryLockError::Error(e) => e,
        })?;
        if let Ok(b) = std::fs::read(data.join("meta.json")) {
            let meta: Value =
                serde_json::from_slice(&b).map_err(|e| refused(format!("{}/meta.json: {e}", data.display())))?;
            if meta["appId"].as_str().is_some_and(|a| a != app) {
                return Err(refused(format!("{} belongs to app {}, not {app}", home.display(), meta["appId"])));
            }
            // A home from before profiles (1.0.9) names none: never refused.
            if meta["profile"].as_str().is_some_and(|p| Some(p) != profile) {
                return Err(refused(format!(
                    "{} is the home of profile {}, not {}",
                    home.display(),
                    meta["profile"],
                    profile.map_or("none".into(), |p| format!("{p:?}"))
                )));
            }
        }
        crate::paths::write_atomic(
            &info,
            json!({ "pid": std::process::id(), "appId": app }).to_string().as_bytes(),
            true,
        )?;
        // A new home says whose it is, as aio's does, so no other app or
        // profile ever opens it; aio's own meta.json is kept as it is.
        let meta = data.join("meta.json");
        if !meta.exists() {
            let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let mut m = json!({ "appId": app, "aio": crate::BUILT_BY, "createdAt": now, "updatedAt": now });
            if let Some(p) = profile {
                m["profile"] = json!(p);
            }
            crate::paths::write_atomic(&meta, m.to_string().as_bytes(), true)?;
        }
        Ok(Home { home: home.into(), data, claim: std::sync::Arc::new(crate::paths::Held(claim)) })
    }

    /// `<home>/data/state.db`, for [`crate::Store::open_aio`].
    pub fn state_db(&self) -> PathBuf {
        self.data.join("state.db")
    }
}

/// Where aio's Android APK keeps key `key` in its store dir `dir`
/// (`filesDir/aio-store`, the activity's `AioNativeStore.fileFor`): the
/// key's readable part (each character outside `[A-Za-z0-9._-]` a `_`, at
/// most 64), `.`, Java's `hashCode` of the key in hex, `.json`.
pub fn android_store_file(dir: &Path, key: &str) -> PathBuf {
    let safe: String =
        key.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).take(64).collect();
    let hash = key.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)));
    dir.join(format!("{safe}.{:x}.json", hash as u32))
}

/// A port's first start on Android, over aio's APK of app `app` (the same
/// application id, so the same files dir `files`): aio's state there
/// (`aio-store`, key `aio:<app>`, the document aio's standalone runtime
/// wrote) made `store`, a `state.db` in aio's layout (`aio_kv`: `state`, and
/// `<app>:__versions` from the document's stamps), which `open_aio` then
/// starts from as it does from aio's desktop file. Once: only while `store`
/// is not there; aio's files are left as they are. Whether it imported; a
/// document that is not aio's (not a JSON object) is refused, never dropped.
pub fn import_android(files: &Path, app: &str, store: &Path) -> io::Result<bool> {
    let from = android_store_file(&files.join("aio-store"), &format!("aio:{app}"));
    if store.exists() || !from.is_file() {
        return Ok(false);
    }
    let bad = |why: String| io::Error::new(io::ErrorKind::InvalidData, why);
    let text = std::fs::read_to_string(&from)?;
    let doc: Value = serde_json::from_str(&text).map_err(|e| bad(format!("{}: {e}", from.display())))?;
    let Some(versions) = doc.as_object().map(|d| d.get("__versions").cloned()) else {
        return Err(bad(format!("{}: aio's state is not an object of cells; left as it is", from.display())));
    };
    if let Some(dir) = store.parent() {
        crate::paths::private_dir(dir)?;
    }
    let making = store.with_extension("importing");
    _ = std::fs::remove_file(&making);
    let made = (|| {
        let db = Connection::open(&making).map_err(|e| format!("storage: {e}"))?;
        db.execute_batch("CREATE TABLE aio_kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)")
            .map_err(|e| format!("storage: {e}"))?;
        db.execute("INSERT INTO aio_kv (k, v) VALUES ('state', ?1)", [&text]).map_err(|e| format!("storage: {e}"))?;
        put(&db, &format!("{app}:__schema"), &json!(SCHEMA))?;
        if let Some(v) = versions.filter(Value::is_object) {
            put(&db, &format!("{app}:__versions"), &v)?;
        }
        db.close().map_err(|(_, e)| format!("storage: {e}"))
    })();
    if let Err(e) = made {
        _ = std::fs::remove_file(&making);
        return Err(io::Error::other(e));
    }
    std::fs::rename(&making, store)?;
    Ok(true)
}

/// Set the files `names` in `data` aside as `<name>.pre-migration`, each
/// with its `-wal` and `-shm` and made private (0600), never deleted: a
/// store the app no longer opens ([`crate::config::Data::retire`], aio's
/// retired `state.db`). A name already set aside is left where it is, so
/// the first copy is never replaced. Returns what was set aside.
pub fn retire(data: &Path, names: &[String]) -> io::Result<Vec<PathBuf>> {
    let mut out = vec![];
    for name in names {
        for ext in ["", "-wal", "-shm"] {
            let (from, to) = (data.join(format!("{name}{ext}")), data.join(format!("{name}{ext}.pre-migration")));
            if !from.is_file() || to.exists() {
                continue;
            }
            #[cfg(unix)]
            std::fs::set_permissions(&from, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
            std::fs::rename(&from, &to)?;
            out.push(to);
        }
    }
    Ok(out)
}

/// A cell field bound to aio's `db:` table: `(field, table, key, columns)`
/// ([`crate::CellData::TABLES`]).
pub type Bound = (&'static str, &'static str, Option<&'static str>, Option<crate::columns::Columns>);

/// A `#[aio_cell]` field ([`crate::CellData::AIO_CELLS`]): aio cell `cell`'s whole slice,
/// kept as the field's own cell type keeps it: its id, version, `migrate`,
/// `#[transient]` and `#[table]` fields.
#[derive(Clone, Copy, Debug)]
pub struct Slice {
    pub field: &'static str,
    pub cell: &'static str,
    pub version: u32,
    pub transient: &'static [&'static str],
    pub include: &'static [&'static str],
    pub tables: &'static [Bound],
    pub migrate: fn(Value, u32) -> Result<Value, String>,
    /// A newer build's slice as this build's type reads it (its fields only).
    pub fit: fn(Value) -> Result<Value, String>,
}

impl Slice {
    /// Field `field`, of cell type `C`.
    pub const fn of<C: crate::CellData>(field: &'static str) -> Self {
        Slice {
            field,
            cell: C::ID,
            version: C::VERSION,
            transient: C::TRANSIENT,
            include: C::PERSIST_INCLUDE,
            tables: C::TABLES,
            migrate: up::<C>,
            fit: fit::<C>,
        }
    }
}

/// A slice read as `C` and written again: `C`'s fields only.
fn fit<C: crate::CellData>(v: Value) -> Result<Value, String> {
    serde_json::from_value::<C>(v).and_then(serde_json::to_value).map_err(|e| e.to_string())
}

/// `C`'s `migrate`, slice to slice.
fn up<C: crate::CellData>(old: Value, from: u32) -> Result<Value, String> {
    serde_json::to_value(C::migrate(old, from)?).map_err(|e| e.to_string())
}

/// A column as `PRAGMA table_info` says it.
#[derive(Clone, Debug, PartialEq)]
struct Column {
    name: String,
    ty: String,
    not_null: bool,
    /// Its default, as SQL text.
    default: Option<String>,
    pk: bool,
}

impl Column {
    /// What `ALTER TABLE … ADD COLUMN` adds.
    fn sql(&self) -> String {
        let mut s = format!("{} {}", quoted(&self.name), self.ty);
        if self.not_null {
            s.push_str(" NOT NULL");
        }
        if let Some(d) = &self.default {
            s.push_str(&format!(" DEFAULT {d}"));
        }
        s
    }

    /// Rows already there get a value: its default, or NULL when allowed.
    fn addable(&self) -> bool {
        !self.pk && (!self.not_null || self.default.is_some())
    }
}

fn table_info(db: &Connection, table: &str) -> Result<Vec<Column>, String> {
    let mut q = db.prepare(&format!("PRAGMA table_info({})", quoted(table))).map_err(|e| format!("storage: {e}"))?;
    q.query_map([], |r| {
        Ok(Column {
            name: r.get(1)?,
            ty: r.get(2)?,
            not_null: r.get(3)?,
            default: r.get(4)?,
            pk: r.get::<_, i64>(5)? > 0,
        })
    })
    .and_then(|r| r.collect())
    .map_err(|e| format!("storage: {e}"))
}

/// SQLite's type affinity (datatype3 §3.1): `INT` and `INTEGER` are one.
fn affinity(ty: &str) -> &'static str {
    let t = ty.to_uppercase();
    match () {
        _ if t.contains("INT") => "INTEGER",
        _ if ["CHAR", "CLOB", "TEXT"].iter().any(|x| t.contains(x)) => "TEXT",
        _ if t.is_empty() || t.contains("BLOB") => "BLOB",
        _ if ["REAL", "FLOA", "DOUB"].iter().any(|x| t.contains(x)) => "REAL",
        _ => "NUMERIC",
    }
}

/// aio's `initSchema` for one bound table: made as `cols` (its SQL column
/// list) declares it, and a stored older shape brought to it wherever no
/// data is lost (aio's `reconcileTable`): a column renamed only in case takes
/// the declared case; a missing column is added, or the whole table made
/// again when it holds no rows (its indexes and triggers with it); a
/// missing one that rows can't fill (NOT NULL without a default), or a
/// stored NOT NULL column without a default that is no longer declared
/// (every write would fail on it), refuses the boot. A changed type or
/// nullability, and a stored column no longer declared, are only said:
/// SQLite keeps what is stored.
pub(crate) fn declare(db: &Connection, table: &str, cols: &str) -> Result<(), String> {
    let refused = |why: String| format!("aio table {table}: {why}");
    let made = format!("CREATE TABLE IF NOT EXISTS {} ({cols})", quoted(table));
    let want = {
        let scratch = Connection::open_in_memory().map_err(|e| format!("storage: {e}"))?;
        scratch.execute_batch(&made).map_err(|e| refused(format!("its columns don't make a table: {e}")))?;
        table_info(&scratch, table)?
    };
    if !has_table(db, table)? {
        return db.execute_batch(&made).map_err(|e| refused(e.to_string()));
    }
    let st = |e: rusqlite::Error| refused(e.to_string());
    let declared = |n: &str| want.iter().any(|c| c.name == n);
    let mut have = table_info(db, table)?;
    for w in &want {
        let Some(s) = have.iter_mut().find(|s| s.name != w.name && s.name.eq_ignore_ascii_case(&w.name)) else {
            continue;
        };
        if declared(&s.name) {
            continue;
        }
        db.execute_batch(&format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {}",
            quoted(table),
            quoted(&s.name),
            quoted(&w.name)
        ))
        .map_err(st)?;
        crate::warn!(cat: "db", "table {table}: column {} renamed to {}, as declared", s.name, w.name);
        s.name = w.name.clone();
    }
    for (w, s) in want.iter().filter_map(|w| Some((w, have.iter().find(|s| s.name == w.name)?))) {
        if affinity(&w.ty) != affinity(&s.ty) {
            crate::warn!(cat: "db", "table {table}: column {} is stored as {} but declared {}; SQLite keeps the stored type", w.name, s.ty, w.ty);
        }
        if !w.pk && w.not_null != s.not_null {
            let (d, k) = if w.not_null { ("NOT NULL", "nullable") } else { ("nullable", "NOT NULL") };
            crate::warn!(cat: "db", "table {table}: column {} is declared {d} but stored {k}; the stored rule is the one in effect", w.name);
        }
    }
    let missing: Vec<&Column> = want.iter().filter(|w| !have.iter().any(|s| s.name == w.name)).collect();
    if !missing.is_empty() {
        let n: i64 = db.query_row(&format!("SELECT COUNT(*) FROM {}", quoted(table)), [], |r| r.get(0)).map_err(st)?;
        if n == 0 {
            return remake(db, table, &made).map_err(refused);
        }
        let blocked: Vec<&str> = missing.iter().filter(|c| !c.addable()).map(|c| c.name.as_str()).collect();
        if !blocked.is_empty() {
            return Err(refused(format!(
                "it lacks column(s) {} and holds {n} row(s), so SQLite has nothing to put in them: give them a \
                 DEFAULT or make them nullable, or migrate the table with aio::db",
                blocked.join(", ")
            )));
        }
        for c in missing {
            db.execute_batch(&format!("ALTER TABLE {} ADD COLUMN {}", quoted(table), c.sql())).map_err(st)?;
            crate::warn!(cat: "db", "table {table}: column {} added", c.name);
        }
    }
    for s in have.iter().filter(|s| !declared(&s.name)) {
        if s.not_null && s.default.is_none() {
            return Err(refused(format!(
                "it has a NOT NULL column {0} the app no longer declares, so every write would fail: declare it again, \
                 or drop it (ALTER TABLE {table} DROP COLUMN {0}); aio never drops a column on its own",
                s.name
            )));
        }
        crate::warn!(cat: "db", "table {table}: column {} is no longer declared; new rows leave it empty", s.name);
    }
    Ok(())
}

/// An empty table made again in its declared shape, its own indexes and
/// triggers kept (one that no longer fits is said and left out).
fn remake(db: &Connection, table: &str, made: &str) -> Result<(), String> {
    let st = |e: rusqlite::Error| e.to_string();
    let mut q = db
        .prepare("SELECT name, sql FROM sqlite_master WHERE tbl_name = ?1 AND type IN ('index', 'trigger') AND sql IS NOT NULL ORDER BY rowid")
        .map_err(st)?;
    let objects: Vec<(String, String)> =
        q.query_map([table], |r| Ok((r.get(0)?, r.get(1)?))).and_then(|r| r.collect()).map_err(st)?;
    let tx = db.unchecked_transaction().map_err(st)?;
    tx.execute_batch(&format!("DROP TABLE {}; {made}", quoted(table))).map_err(st)?;
    for (name, sql) in objects {
        if let Err(e) = tx.execute_batch(&sql) {
            crate::warn!(cat: "db", "table {table}: made again, but {name} no longer fits it ({e}): {sql}");
        }
    }
    tx.commit().map_err(st)?;
    crate::warn!(cat: "db", "table {table}: empty and its columns changed; made again as declared");
    Ok(())
}

/// aio's record of the bound tables it has written once
/// (`state-sync.ts`): an empty one of them is empty on purpose.
const SYNCED: &str = "aio_synced_tables";

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A column as a bound row's value goes in and comes out of it: a `bool` as
/// 0/1 in a `BOOLEAN` column, a list or an object as JSON text in a `JSON`
/// one, each value as its JSON text (aio-rs's [`crate::columns::boolean`],
/// [`crate::columns::json`]); in any other, a number, a string or null, as
/// aio stores them.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Plain,
    Bool,
    Json,
}

impl Kind {
    fn of(ty: &str) -> Self {
        match ty.to_ascii_uppercase().as_str() {
            "BOOLEAN" => Kind::Bool,
            "JSON" => Kind::Json,
            _ => Kind::Plain,
        }
    }
}

/// One of a table's columns: its name, whether it is the primary key, its kind.
type Col = (String, bool, Kind);

/// Table `table`'s columns; `None`: no such table.
fn columns(db: &Connection, table: &str) -> Result<Option<Vec<Col>>, String> {
    if !has_table(db, table)? {
        return Ok(None);
    }
    let mut q = db.prepare(&format!("PRAGMA table_info({})", quoted(table))).map_err(|e| format!("storage: {e}"))?;
    let cols = q
        .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(5)? == 1, Kind::of(&r.get::<_, String>(2)?))))
        .and_then(|r| r.collect::<Result<Vec<_>, _>>())
        .map_err(|e| format!("storage: {e}"))?;
    Ok(Some(cols))
}

fn synced(db: &Connection, table: &str) -> Result<bool, String> {
    if !has_table(db, SYNCED)? {
        return Ok(false);
    }
    db.query_row(&format!("SELECT EXISTS (SELECT 1 FROM {SYNCED} WHERE name = ?1)"), [table], |r| r.get(0))
        .map_err(|e| format!("storage: {e}"))
}

/// Every row of `table` (columns `cols`) as aio reads it (`SELECT *`): an
/// object of its columns, numbers, strings and nulls (a blob as its bytes);
/// a `BOOLEAN` column's 0/1 as a bool, a `JSON` column's text as its value.
fn rows(db: &Connection, table: &str, cols: &[Col]) -> Result<Vec<Value>, String> {
    use rusqlite::types::ValueRef;
    let err = |e: rusqlite::Error| format!("aio table {table}: {e}");
    let mut q = db.prepare(&format!("SELECT * FROM {}", quoted(table))).map_err(err)?;
    let names: Vec<(String, Kind)> = q
        .column_names()
        .into_iter()
        .map(|n| (n.to_string(), cols.iter().find(|c| c.0 == n).map_or(Kind::Plain, |c| c.2)))
        .collect();
    let rows = q
        .query_map([], |r| {
            let mut m = Map::new();
            for (i, (n, kind)) in names.iter().enumerate() {
                let v = match (r.get_ref(i)?, kind) {
                    (ValueRef::Integer(b @ (0 | 1)), Kind::Bool) => json!(b == 1),
                    (ValueRef::Text(t), Kind::Json) => {
                        serde_json::from_slice(t).unwrap_or_else(|_| json!(String::from_utf8_lossy(t)))
                    }
                    (v, _) => plain(v),
                };
                m.insert(n.clone(), v);
            }
            Ok(Value::Object(m))
        })
        .and_then(|r| r.collect::<Result<Vec<_>, _>>())
        .map_err(err)?;
    Ok(rows)
}

/// A stored value as aio reads it.
fn plain(v: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => json!(n),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => json!(b),
    }
}

/// A map binding's key for a row's key value (aio's `String(row[pk])`).
fn key_of(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), String::from)
}

/// A bound row, with its key when the binding has one.
type Row = (Option<String>, Map<String, Value>);

/// The rows a bound field holds, by key when it has one (the map's key
/// must be its row's, as aio checks).
fn rows_of(v: &Value, key: Option<&str>) -> Result<Vec<Row>, String> {
    let row = |r: &Value| r.as_object().cloned().ok_or_else(|| format!("a row is {r}, not an object"));
    match v {
        Value::Null => Ok(vec![]),
        Value::Array(a) => a.iter().map(|r| Ok((key.map(|k| key_of(&r[k])), row(r)?))).collect(),
        Value::Object(m) => m
            .iter()
            .map(|(k, r)| match key {
                Some(col) if key_of(&r[col]) == *k => Ok((Some(k.clone()), row(r)?)),
                Some(col) => Err(format!("the row under {k:?} has {col} = {}", r[col])),
                None => Err("a map needs #[table(\"name\", key = \"col\")]".into()),
            })
            .collect(),
        v => Err(format!("{v} is not rows")),
    }
}

/// Write `new`'s rows into `table` (columns `cols`): with a key and the
/// rows synced before (`old`), only what changed; else all of them. A row
/// aio couldn't store (a column the table lacks, a value SQLite can't hold)
/// refuses the commit, as aio refuses it.
fn sync(
    db: &Connection,
    table: &str,
    cols: &[Col],
    key: Option<&str>,
    old: Option<&Value>,
    new: &Value,
) -> Result<(), String> {
    let pk = key.or_else(|| cols.iter().find(|c| c.1).map(|c| c.0.as_str()));
    let keyed = |v: &Value| rows_of(v, pk);
    let new = keyed(new)?;
    let st = |e: rusqlite::Error| format!("storage: {e}");
    let upsert = |r: &Map<String, Value>| -> Result<(), String> {
        let mut names = vec![];
        let mut vals: Vec<rusqlite::types::Value> = vec![];
        for (k, v) in r {
            let Some(col) = cols.iter().find(|c| c.0 == *k) else {
                return Err(format!("the table has no column {k}"));
            };
            names.push(quoted(k));
            vals.push(match (v, col.2) {
                (Value::Null, _) => rusqlite::types::Value::Null,
                // Every value as its JSON text: a string reads back a string.
                (v, Kind::Json) => v.to_string().into(),
                (Value::String(s), _) => s.clone().into(),
                (Value::Number(n), _) => match n.as_i64() {
                    Some(i) => i.into(),
                    None => n.as_f64().unwrap_or(f64::NAN).into(),
                },
                (Value::Bool(b), Kind::Bool) => i64::from(*b).into(),
                (v, _) => {
                    return Err(format!(
                        "{k} = {v}: a column holds a number, a string or null (as in aio); a bool needs a BOOLEAN \
                         column and a list or an object a JSON one (aio::columns::boolean, json)"
                    ));
                }
            });
        }
        let marks = vec!["?"; names.len()].join(", ");
        let sql = format!("INSERT OR REPLACE INTO {} ({}) VALUES ({marks})", quoted(table), names.join(", "));
        db.execute(&sql, rusqlite::params_from_iter(vals)).map(drop).map_err(st)
    };
    match (pk, old) {
        (Some(pk), Some(old)) => {
            let old: std::collections::HashMap<_, _> = keyed(old)?.into_iter().collect();
            let kept: std::collections::HashSet<&Option<String>> = new.iter().map(|(k, _)| k).collect();
            for gone in old.keys().filter(|k| !kept.contains(k)).flatten() {
                db.execute(&format!("DELETE FROM {} WHERE {} = ?1", quoted(table), quoted(pk)), [gone]).map_err(st)?;
            }
            for (_, r) in new.iter().filter(|(k, r)| old.get(k) != Some(r)) {
                upsert(r)?;
            }
        }
        _ => {
            db.execute(&format!("DELETE FROM {}", quoted(table)), []).map_err(st)?;
            for (_, r) in &new {
                upsert(r)?;
            }
        }
    }
    db.execute_batch(&format!("CREATE TABLE IF NOT EXISTS {SYNCED} (name TEXT PRIMARY KEY NOT NULL)")).map_err(st)?;
    db.execute(&format!("INSERT OR IGNORE INTO {SYNCED} (name) VALUES (?1)"), [table]).map_err(st)?;
    Ok(())
}

/// The bound fields' rows, by field, as aio places them: of each table the
/// file has, but an empty one never synced (the field keeps its own value).
pub(crate) fn bound(db: &Connection, tables: &[Bound]) -> Result<Map<String, Value>, String> {
    let mut got = Map::new();
    for &(field, table, key, _) in tables {
        let Some(cols) = columns(db, table)? else { continue };
        let rows = rows(db, table, &cols)?;
        if rows.is_empty() && !synced(db, table)? {
            continue;
        }
        let v = match key {
            Some(k) => Value::Object(rows.into_iter().map(|r| (key_of(&r[k]), r)).collect()),
            None => Value::Array(rows),
        };
        got.insert(field.into(), v);
    }
    Ok(got)
}

/// Cell `cell`'s bound fields taken out of `state` into the tables the file
/// has (what changed since `prev`, by key), each table marked synced; a
/// field without its table stays in `state`.
pub(crate) fn write_bound(
    db: &Connection,
    cell: &str,
    state: &mut Value,
    prev: Option<&Value>,
    tables: &[Bound],
) -> Result<(), String> {
    for &(field, table, key, _) in tables {
        let Some(cols) = columns(db, table)? else { continue };
        let new = state.as_object_mut().and_then(|m| m.remove(field)).unwrap_or(Value::Null);
        let old = prev.and_then(|p| p.get(field)).filter(|_| synced(db, table).unwrap_or(false));
        if old != Some(&new) {
            sync(db, table, &cols, key, old, &new).map_err(|e| format!("{cell}.{field} (aio table {table}): {e}"))?;
        }
    }
    Ok(())
}

/// Where one app's cells live in `aio_kv`: found at open, kept by every write.
#[derive(Clone, Debug)]
pub(crate) struct Kv {
    app: String,
    /// aio's `persistKey` (`state` unless stamped otherwise).
    key: String,
    /// One row per cell (`<key>\x1f<cell>`), else one document row `<key>`.
    multi: bool,
}

pub(crate) const KV_TABLE: &str = "CREATE TABLE IF NOT EXISTS aio_kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)";

fn get(db: &Connection, k: &str) -> Result<Option<Value>, String> {
    let text: Option<String> = db
        .query_row("SELECT v FROM aio_kv WHERE k = ?1", [k], |r| r.get(0))
        .optional()
        .map_err(|e| format!("storage: {e}"))?;
    text.map(|t| serde_json::from_str(&t).map_err(|e| format!("aio_kv row {k:?} is not JSON: {e}"))).transpose()
}

fn put(db: &Connection, k: &str, v: &Value) -> Result<(), String> {
    db.execute(
        "INSERT INTO aio_kv (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![k, v.to_string()],
    )
    .map(|_| ())
    .map_err(|e| format!("storage: {e}"))
}

fn has_table(db: &Connection, name: &str) -> Result<bool, String> {
    db.query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)", [name], |r| r.get(0))
        .map_err(|e| format!("storage: {e}"))
}

/// The method of `T` that aio's `<cell>:<method>` is (pure): `T`'s own
/// under its id; under a `#[aio_cell]` field's cell `<cell>_<method>` when `T`
/// has it, else `<method>`. `None`: no cell of `T`'s, or an internal method.
pub(crate) fn method_of<T: crate::Cell>(cell: &str, method: &str) -> Option<String> {
    if method.is_empty() || method.starts_with("__") {
        return None;
    }
    if cell == T::ID {
        return Some(method.into());
    }
    T::AIO_CELLS.iter().any(|s| s.cell == cell).then(|| {
        let own = format!("{cell}_{method}");
        if T::METHODS.contains(&own.as_str()) { own } else { method.into() }
    })
}

/// aio's journal lines that carry no change to a cell (`journal.ts`).
const JOURNAL_NO_OPS: [&str; 2] = ["__aioSyncIntent", "__aioSyncApplied"];

/// One aio journal entry past the store's watermark, as aio-rs replays it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Journaled {
    /// `<cell>:<method>`: the call, its arguments, who made it, when.
    Call { seq: i64, method: String, args: Value, by: crate::By, at: i64 },
    /// No change to replay: aio's own bookkeeping, a call on a cell that saves
    /// nothing, or a call whose arguments aio withheld (said).
    Skip { seq: i64, why: Option<String> },
}

/// What `<data>/journal` (aio's `journal.ts`, one JSON entry a line) holds past
/// `wm` (the store's `__journal_wm`) for cell type `T`, in order; empty when
/// it holds nothing of `T`'s cells. Refused when aio-rs can't replay it as
/// aio would: a store older than what the journal already dropped
/// (`journal.base`), an entry that isn't a plain call (a time-travel or
/// write-set line, an action without a cell), or a tail mixing `T`'s calls
/// with another cell's (only aio could then move the watermark). A torn line
/// (a crash mid-append) is skipped, as aio does.
pub(crate) fn journal_tail<T: crate::Cell>(data: &Path, wm: i64) -> Result<Vec<Journaled>, String> {
    let text = match std::fs::read_to_string(data.join("journal")) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("{}/journal: {e}", data.display())),
    };
    if let Ok(b) = std::fs::read(data.join("journal.base"))
        && let Some(dropped) = serde_json::from_slice::<Value>(&b).ok().and_then(|v| v["wm"].as_i64())
        && dropped > wm
    {
        return Err(format!(
            "its journal dropped calls up to {dropped} but the store holds only up to {wm} (the store was put back to an older copy)"
        ));
    }
    let mut lines = vec![];
    for (i, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        match serde_json::from_str::<Value>(line) {
            Ok(e) if e["type"] == "__aioBatch" => lines.extend(e["entries"].as_array().cloned().unwrap_or_default()),
            Ok(e) => lines.push(e),
            Err(_) => eprintln!(
                "[aio] data: {}/journal line {} is torn (a crash mid-write): skipped, as aio does",
                data.display(),
                i + 1
            ),
        }
    }
    let (mut ours, mut others) = (vec![], vec![]);
    for e in lines {
        let Some(seq) = e["seq"].as_i64().filter(|s| *s > wm) else { continue };
        let kind = e["type"].as_str().unwrap_or_default();
        let Some((cell, m)) = kind.split_once(':') else {
            match JOURNAL_NO_OPS.contains(&kind) {
                true => ours.push(Journaled::Skip { seq, why: None }),
                false => return Err(format!("journal entry {seq} ({kind}) is no call aio-rs can replay")),
            }
            continue;
        };
        let Some(method) = method_of::<T>(cell, m) else {
            match m.starts_with("__") && (cell == T::ID || T::AIO_CELLS.iter().any(|s| s.cell == cell)) || cell == "aio"
            {
                true => {
                    return Err(format!("journal entry {seq} ({kind}) carries state, not a call aio-rs can replay"));
                }
                false => others.push(kind.to_string()),
            }
            continue;
        };
        let j = match (e["unstored"] == true, e["redacted"] == true) {
            (true, _) => Journaled::Skip { seq, why: None },
            (_, true) => Journaled::Skip {
                seq,
                why: Some(format!(
                    "journal entry {seq} ({kind}): aio withheld its arguments (redactActions), so it can't run again: skipped, as aio does"
                )),
            },
            _ => Journaled::Call {
                seq,
                method,
                args: match &e["payload"]["args"] {
                    Value::Null => json!([]),
                    a => a.clone(),
                },
                by: e["user"]["id"].as_str().map_or(crate::By::System, |u| crate::By::User(u.into())),
                at: e["ts"].as_i64().unwrap_or(0),
            },
        };
        ours.push(j);
    }
    let calls = ours.iter().any(|j| matches!(j, Journaled::Call { .. }));
    match (calls, others.first()) {
        (true, Some(other)) => Err(format!("its journal holds calls of {} and of another cell ({other})", T::ID)),
        (false, _) => Ok(vec![]),
        (true, None) => Ok(ours),
    }
}

impl Kv {
    /// aio's journal watermark (`<app>:__journal_wm`): the last journal
    /// entry the snapshot holds (0: none).
    pub(crate) fn journal_wm(&self, db: &Connection) -> Result<i64, String> {
        Ok(get(db, &format!("{}:__journal_wm", self.app))?.and_then(|v| v.as_i64()).unwrap_or(0))
    }

    /// Move aio's journal watermark to `seq`, in `db`'s transaction (with the
    /// snapshot that holds the entries up to it, as aio writes it).
    pub(crate) fn set_journal_wm(&self, db: &Connection, seq: i64) -> Result<(), String> {
        put(db, &format!("{}:__journal_wm", self.app), &json!(seq))
    }

    /// Read only: app `app`'s snapshot layout, or what makes aio refuse the
    /// file (aio-rs refuses it too). A file without `aio_kv` gets aio's defaults.
    pub(crate) fn find(db: &Connection, app: &str) -> Result<Self, String> {
        let fresh = Kv { app: app.into(), key: "state".into(), multi: false };
        if !has_table(db, "aio_kv")? {
            return Ok(fresh);
        }
        let schema = get(db, &format!("{app}:__schema"))?.and_then(|v| v.as_i64()).unwrap_or(0);
        if schema > SCHEMA {
            return Err(format!("{app}: aio data from a newer aio (schema {schema} > {SCHEMA})"));
        }
        if has_table(db, "aio_schema")? {
            let v: Option<i64> = db
                .query_row("SELECT version FROM aio_schema WHERE id = 1", [], |r| r.get(0))
                .optional()
                .map_err(|e| format!("storage: {e}"))?;
            if v.is_some_and(|v| v > SCHEMA) {
                return Err(format!("{app}: aio tables from a newer aio (aio_schema {} > {SCHEMA})", v.unwrap_or(0)));
            }
        }
        let key =
            get(db, &format!("{app}:__persistKey"))?.and_then(|v| v.as_str().map(String::from)).unwrap_or(fresh.key);
        let layout = get(db, &format!("{app}:__layout"))?;
        let single = get(db, &key)?;
        let rows: bool = db
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM aio_kv WHERE k >= ?1 AND k < ?2)",
                params![format!("{key}{SEP}"), format!("{key}\x20")],
                |r| r.get(0),
            )
            .map_err(|e| format!("storage: {e}"))?;
        // aio's rule: the stamped layout, else the document row when it is
        // one, else the per-cell rows.
        let multi = match layout.as_ref().and_then(Value::as_str) {
            Some("multi") => true,
            Some("single") => false,
            _ => rows && !single.as_ref().is_some_and(Value::is_object),
        };
        if let Some(doc) = single.filter(|d| !multi && !d.is_object()) {
            let doc: String = doc.to_string().chars().take(80).collect();
            return Err(format!("{app}: \"{key}\" holds {doc}, not an object of cells"));
        }
        Ok(Kv { app: app.into(), key, multi })
    }

    /// Cell `cell`'s stored slice, raw (with its `__*` keys).
    fn raw(&self, db: &Connection, cell: &str) -> Result<Option<Value>, String> {
        Ok(if self.multi {
            get(db, &format!("{}{SEP}{cell}", self.key))?
        } else {
            get(db, &self.key)?.and_then(|mut d| d.get_mut(cell).map(Value::take))
        })
    }

    /// Has a downgrade boot parked cell `cell`'s slice (`__downgraded:<cell>`)?
    fn parked(&self, db: &Connection, cell: &str) -> Result<bool, String> {
        let park = format!("__downgraded:{cell}");
        Ok(match self.multi {
            true => get(db, &format!("{}{SEP}{park}", self.key))?.is_some(),
            false => get(db, &self.key)?.is_some_and(|d| d.get(&park).is_some()),
        })
    }

    fn versions(&self, db: &Connection) -> Result<Map<String, Value>, String> {
        Ok(get(db, &format!("{}:__versions", self.app))?.and_then(|v| v.as_object().cloned()).unwrap_or_default())
    }

    /// Cell `cell`'s state as aio saved it, without the `__*` keys aio never
    /// restores, and its version stamp (none: 0, as aio reads it). A field
    /// bound to a `db:` table (`tables`) is that table's rows, as aio places
    /// them: an empty table aio never synced leaves the slice's own array.
    pub(crate) fn slice(&self, db: &Connection, cell: &str, tables: &[Bound]) -> Result<Option<(Value, u32)>, String> {
        if !has_table(db, "aio_kv")? {
            return Ok(None);
        }
        let mut st = self.raw(db, cell)?;
        for (field, v) in bound(db, tables)? {
            st.get_or_insert_with(|| json!({}))[field] = v;
        }
        let Some(mut st) = st else { return Ok(None) };
        let parked = self.parked(db, cell)?;
        if let Some(m) = st.as_object_mut() {
            m.retain(|k, _| !k.starts_with("__"));
            // An older build ran on it (aio's downgrade park): see `store::DOWNGRADED`.
            if parked {
                m.insert(crate::store::DOWNGRADED.into(), json!(true));
            }
        }
        let v = self.versions(db)?.get(cell).and_then(Value::as_u64).unwrap_or(0);
        Ok(Some((st, u32::try_from(v).unwrap_or(u32::MAX))))
    }

    /// The slices of `of` the file has, by field, each brought to its cell's
    /// version (`migrate`; a newer one refuses), and the first one that was
    /// older (`(cell, stamp)`, for the backup). `None`: the file has none.
    #[allow(clippy::type_complexity)]
    pub(crate) fn slices(
        &self,
        db: &Connection,
        of: &[Slice],
    ) -> Result<Option<(Map<String, Value>, Option<(&'static str, u32)>)>, String> {
        let (mut got, mut older) = (Map::new(), None);
        for s in of {
            let Some((mut v, at)) = self.slice(db, s.cell, s.tables)? else { continue };
            let downgraded = v.as_object_mut().and_then(|m| m.remove(crate::store::DOWNGRADED)).is_some();
            if at < s.version {
                older = older.or(Some((s.cell, at)));
            }
            let v = match at.cmp(&s.version) {
                std::cmp::Ordering::Less => {
                    (s.migrate)(v, at).map_err(|e| format!("{}: migrate from v{at} failed: {e}", s.cell))?
                }
                // A newer build's (a rollback): what this build declares; each
                // write keeps the rest (`Kv::write`).
                std::cmp::Ordering::Greater => {
                    eprintln!(
                        "[aio] {}: aio data from a newer build (v{at} > v{}): it boots, keeping that build's fields it doesn't declare",
                        s.cell, s.version
                    );
                    (s.fit)(v).map_err(|e| {
                        format!("{}: aio data from a newer build (v{at} > v{}) doesn't fit: {e}", s.cell, s.version)
                    })?
                }
                // Back from a rollback: the older build's own fields go, said
                // (aio's restore merge drops them).
                std::cmp::Ordering::Equal if downgraded => {
                    let fit = (s.fit)(v.clone()).map_err(|e| format!("{}: {e}", s.cell))?;
                    let gone: Vec<&str> = v
                        .as_object()
                        .into_iter()
                        .flat_map(|m| m.keys())
                        .filter(|k| fit.get(k.as_str()).is_none())
                        .map(String::as_str)
                        .collect();
                    if !gone.is_empty() {
                        eprintln!(
                            "[aio] {}: an older build ran on this data and wrote fields this build doesn't declare ({}); they are dropped, as aio drops them",
                            s.cell,
                            gone.join(", ")
                        );
                    }
                    fit
                }
                std::cmp::Ordering::Equal => v,
            };
            got.insert(s.field.into(), v);
        }
        Ok((!got.is_empty()).then_some((got, older)))
    }

    /// Cells `gone` taken out of the file, in `db`'s transaction: their
    /// slices (either layout) and `__versions` stamps. Whether one was there.
    pub(crate) fn retire(&self, db: &Connection, gone: &[&str]) -> Result<bool, String> {
        let st = |e: rusqlite::Error| format!("storage: {e}");
        let mut was = false;
        for c in gone {
            was |= db.execute("DELETE FROM aio_kv WHERE k = ?1", [format!("{}{SEP}{c}", self.key)]).map_err(st)? > 0;
        }
        if let Some(Value::Object(mut doc)) = get(db, &self.key)? {
            let n = doc.len();
            gone.iter().for_each(|c| _ = doc.remove(*c));
            if doc.len() < n {
                put(db, &self.key, &Value::Object(doc))?;
                was = true;
            }
        }
        let mut versions = self.versions(db)?;
        let n = versions.len();
        gone.iter().for_each(|c| _ = versions.remove(*c));
        if versions.len() < n {
            put(db, &format!("{}:__versions", self.app), &Value::Object(versions))?;
            was = true;
        }
        Ok(was)
    }

    /// Write every slice of `of` that `state` changed since `prev` back as
    /// [`Kv::write`] does, each at its cell's version, in `db`'s transaction.
    pub(crate) fn write_slices(
        &self,
        db: &Connection,
        of: &[Slice],
        state: &Value,
        prev: &Value,
    ) -> Result<(), String> {
        for s in of {
            // Compared before it is copied: most commits change one slice.
            let (new, old) = (state.get(s.field), prev.get(s.field));
            if old != new || new.is_none() {
                let new = new.cloned().unwrap_or_else(|| json!({}));
                if old != Some(&new) {
                    self.write(db, s.cell, s.version, new, old, s.tables)?;
                }
            }
        }
        Ok(())
    }

    /// Write cell `cell` (at `version`) back as aio does: its slice in the
    /// file's layout with its stored `__*` keys kept, the other cells and
    /// keys as they were, and the stamps (`__schema`, `__persistKey`, a
    /// monotonic `__versions`; a version 0 cell is not stamped, as in aio).
    /// A field bound to a `db:` table that the file has goes into its rows
    /// instead (what changed since `prev`, by key), the table marked synced;
    /// without the table it stays in the slice, which aio then adopts.
    pub(crate) fn write(
        &self,
        db: &Connection,
        cell: &str,
        version: u32,
        mut state: Value,
        prev: Option<&Value>,
        tables: &[Bound],
    ) -> Result<(), String> {
        write_bound(db, cell, &mut state, prev, tables)?;
        let mut versions = self.versions(db)?;
        let stored = versions.get(cell).and_then(Value::as_u64).unwrap_or(0);
        let old = self.raw(db, cell)?;
        if let (Some(old), Some(m)) = (&old, state.as_object_mut()) {
            for (k, v) in old.as_object().into_iter().flatten().filter(|(k, _)| k.starts_with("__")) {
                m.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        // A newer build's slice (a rollback, aio's downgrade): its fields this
        // build doesn't know stay, and a verbatim copy is parked once under
        // `__downgraded:<cell>`, as aio parks it.
        let newer = stored > u64::from(version);
        if let (true, Some(old)) = (newer, &old) {
            crate::filter::reattach(&mut state, old);
        }
        let park = format!("__downgraded:{cell}");
        if self.multi {
            put(db, &format!("{}{SEP}{cell}", self.key), &state)?;
            let parked = format!("{}{SEP}{park}", self.key);
            if let (true, Some(old), None) = (newer, &old, get(db, &parked)?) {
                put(db, &parked, old)?;
            }
        } else {
            let mut doc = get(db, &self.key)?.filter(Value::is_object).unwrap_or_else(|| json!({}));
            if let (true, Some(old), None) = (newer, &old, doc.get(&park)) {
                doc[&park] = old.clone();
            }
            doc[cell] = state;
            put(db, &self.key, &doc)?;
        }
        put(db, &format!("{}:__schema", self.app), &json!(SCHEMA))?;
        put(db, &format!("{}:__persistKey", self.app), &json!(self.key))?;
        if u64::from(version) > stored {
            versions.insert(cell.into(), json!(version));
            put(db, &format!("{}:__versions", self.app), &Value::Object(versions))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn at(
        app: &str,
        home: Option<&str>,
        profile: Option<&str>,
        apps: Option<&str>,
    ) -> Result<(String, Option<String>), String> {
        resolve(app, home, profile, apps.map(Path::new), Path::new("/u")).map(|(p, n)| (p.display().to_string(), n))
    }

    #[test]
    #[cfg(unix)]
    fn homes_resolve_as_in_aio() {
        assert_eq!(at("notes", None, None, None).unwrap(), ("/u/.notes".into(), None));
        assert_eq!(at("notes", None, None, Some("/srv/aio")).unwrap(), ("/srv/aio/notes".into(), None));
        assert_eq!(at("notes", None, Some("dev"), None).unwrap(), ("/u/.notes-dev".into(), Some("dev".into())));
        assert_eq!(
            at("notes", None, Some("dev"), Some("/srv")).unwrap(),
            ("/srv/notes-dev".into(), Some("dev".into()))
        );
        // A path names that folder; the folder of a profile is that profile.
        assert_eq!(at("notes", None, Some("~/.notes-qa"), None).unwrap(), ("/u/.notes-qa".into(), Some("qa".into())));
        assert_eq!(at("notes", Some("/data/n"), None, None).unwrap(), ("/data/n".into(), None));
        assert_eq!(at("notes", Some("/u/.notes-dev"), Some("dev"), None).unwrap().1, Some("dev".into()));
        assert!(at("notes", Some("/x"), Some("dev"), None).unwrap_err().contains("two different folders"));
        assert!(at("notes", Some("  "), None, None).unwrap_err().contains("empty"));
        for bad in ["Dev", "-x", "default", "1a2b3c4d", &"a".repeat(33)] {
            assert!(at("notes", None, Some(bad), None).is_err(), "{bad}");
        }
    }

    #[test]
    fn another_programs_folder_is_refused_as_in_aio() {
        let d = tempfile::tempdir().unwrap();
        let (user, home) = (d.path(), d.path().join(".notes"));
        // Absent, empty, desktop litter only, or aio's: an app's home.
        assert_eq!(refusal("notes", &home, user), None);
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join(".DS_Store"), b"").unwrap();
        std::fs::write(home.join("Thumbs.db"), b"").unwrap();
        assert_eq!(refusal("notes", &home, user), None);
        for f in ["c", "a", "b", "d"] {
            std::fs::write(home.join(f), b"").unwrap();
        }
        let e = refusal("notes", &home, user).unwrap();
        assert!(e.contains("another program's files (a, b, c, …1 more)"), "{e}");
        std::fs::create_dir(home.join("logs")).unwrap();
        assert_eq!(refusal("notes", &home, user), None, "aio's own entry: an app's");
        let file = d.path().join("f");
        std::fs::write(&file, b"").unwrap();
        assert!(refusal("notes", &file, user).unwrap().contains("a file"));
        // `~/.ssh` is never an app's, made yet or not; an app already there is.
        let ssh = user.join(".ssh");
        assert!(refusal("ssh", &ssh, user).unwrap().contains("another program's"));
        assert!(!ssh.exists());
        assert_eq!(refusal("ssh", &d.path().join("apps").join("ssh"), user), None, "only ~/.<name>");
        std::fs::create_dir_all(ssh.join("data")).unwrap();
        std::fs::create_dir_all(ssh.join("logs")).unwrap();
        assert_eq!(refusal("ssh", &ssh, user), None);
    }

    #[test]
    fn profile_paths_are_told_from_names() {
        for p in ["./x", "~", "~/x", "a/b", "a\\b", "C:x", ".x"] {
            assert!(is_path(p), "{p}");
        }
        for n in ["dev", "test-2", "c"] {
            assert!(!is_path(n), "{n}");
        }
    }
}

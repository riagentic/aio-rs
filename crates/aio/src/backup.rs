//! Backups of an app's store (D11): a consistent copy made while the app
//! runs (`VACUUM INTO`, as `.partial`, flushed, then renamed), stamped with
//! the app's id. A restore refuses another app's backup and a running app,
//! and sets the data it replaces aside (never deletes it).
//!
//! [`backup`] holds the store only (what an update sets aside). [`backup_all`]
//! (`ax backup`) holds the rest of `data/` too, inside the same file: the
//! app's files and other databases (each copied consistently), all but what
//! belongs to one machine or one run (keys, the lock, health) or is made
//! again (the window's browser profile, update backups).
use rusqlite::{Connection, OpenFlags};
use std::io;
use std::path::{Path, PathBuf};

fn err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

fn with_suffix(p: &Path, s: &str) -> PathBuf {
    let mut o = p.as_os_str().to_owned();
    o.push(s);
    o.into()
}

/// Copy the store at `store` (app `id`) to `to`.
pub fn backup(id: &str, store: &Path, to: &Path) -> io::Result<()> {
    make(id, store, to, false)
}

/// Copy the store at `store` (app `id`) and the rest of its data dir to `to`.
pub fn backup_all(id: &str, store: &Path, to: &Path) -> io::Result<()> {
    make(id, store, to, true)
}

/// What in `data/` a whole backup leaves out (`path`: relative, `/`-separated).
pub fn left_out(path: &str) -> bool {
    const OUT: [&str; 7] =
        ["instance.lock", "health.json", "serving.json", "sync.token", "paired.json", "electron", "backups"];
    let (top, name) = (path.split('/').next().unwrap_or(path), path.rsplit('/').next().unwrap_or(path));
    OUT.contains(&top) || top.starts_with("state.db") || name.ends_with(".tmp") || name.contains(".replaced-")
}

/// The store's file name (`state.db`, or the app's `[data] store`).
fn name(store: &Path) -> String {
    store.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Every file under `dir` a whole backup holds, relative, `/`-separated:
/// not the store `store` (a file name in `dir`) and its copies.
fn files(dir: &Path, at: &str, store: &str, out: &mut Vec<String>) -> io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let rel = format!("{at}{}", e.file_name().to_string_lossy());
        let kind = e.file_type()?;
        if left_out(&rel)
            || (at.is_empty() && rel.starts_with(store))
            || kind.is_symlink()
            || rel.ends_with("-wal")
            || rel.ends_with("-shm")
        {
            continue;
        }
        if kind.is_dir() {
            files(&e.path(), &format!("{rel}/"), store, out)?;
        } else {
            out.push(rel);
        }
    }
    Ok(())
}

/// The bytes of `file`: a SQLite database copied consistently (its WAL in).
fn bytes_of(file: &Path, scratch: &Path) -> io::Result<Vec<u8>> {
    let raw = std::fs::read(file)?;
    if !raw.starts_with(b"SQLite format 3\0") {
        return Ok(raw);
    }
    _ = std::fs::remove_file(scratch);
    let db = Connection::open_with_flags(file, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    db.execute("VACUUM INTO ?1", [scratch.to_string_lossy()]).map_err(err)?;
    drop(db);
    let b = std::fs::read(scratch);
    _ = std::fs::remove_file(scratch);
    b
}

fn make(id: &str, store: &Path, to: &Path, all: bool) -> io::Result<()> {
    if !store.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{id} has no store at {}", store.display())));
    }
    if to.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} exists: pick a new name", to.display())));
    }
    let partial = with_suffix(to, ".partial");
    _ = std::fs::remove_file(&partial);
    let src = Connection::open_with_flags(store, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    src.execute("VACUUM INTO ?1", [partial.to_string_lossy()]).map_err(err)?;
    let made = (|| {
        let db = Connection::open(&partial).map_err(err)?;
        db.execute_batch("CREATE TABLE aio_backup (app TEXT NOT NULL, made INTEGER NOT NULL)").map_err(err)?;
        db.execute("INSERT INTO aio_backup VALUES (?1, ?2)", rusqlite::params![id, crate::store::now_ms()])
            .map_err(err)?;
        if all {
            let data = crate::paths::parent(store);
            let mut list = Vec::new();
            files(data, "", &name(store), &mut list)?;
            db.execute_batch("CREATE TABLE aio_files (path TEXT PRIMARY KEY, bytes BLOB NOT NULL)").map_err(err)?;
            let scratch = with_suffix(&partial, ".db");
            for f in list {
                let b = bytes_of(&data.join(&f), &scratch)?;
                db.execute("INSERT INTO aio_files VALUES (?1, ?2)", rusqlite::params![f, b]).map_err(err)?;
            }
        }
        drop(db);
        crate::paths::sync_file(&partial)?;
        crate::paths::rename_over(&partial, to)?;
        crate::paths::sync_dir(crate::paths::parent(to))
    })();
    if made.is_err() {
        _ = std::fs::remove_file(&partial);
    }
    made
}

/// The app a backup file was made for.
pub fn app_of(file: &Path) -> io::Result<String> {
    let db = Connection::open_with_flags(file, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    db.query_row("SELECT app FROM aio_backup", [], |r| r.get(0))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("{} is not an aio-rs backup", file.display())))
}

/// Move `f` (with a database's `-wal` and `-shm`) to `f<mark>`; that path,
/// if there was a file.
fn set_aside(f: &Path, mark: &str) -> io::Result<Option<PathBuf>> {
    let aside = with_suffix(f, mark);
    let had = f.exists();
    for ext in ["", "-wal", "-shm"] {
        let from = with_suffix(f, ext);
        if from.exists() {
            std::fs::rename(from, with_suffix(&aside, ext))?;
        }
    }
    Ok(had.then_some(aside))
}

/// Put backup `file` in place of app `id`'s store at `store`. The app must
/// not run (`dirs` is its instance lock); the store it replaces is kept as
/// `<store>.replaced-<ms>`. Returns that path, if there was a store.
pub fn restore(id: &str, file: &Path, store: &Path, _dirs: &crate::paths::AppDirs) -> io::Result<Option<PathBuf>> {
    let app = app_of(file)?;
    if app != id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is a backup of {app}, not {id}", file.display()),
        ));
    }
    // Restore a copy, so the backup itself stays as it was.
    let incoming = with_suffix(store, ".restoring");
    _ = std::fs::remove_file(&incoming);
    std::fs::copy(file, &incoming)?;
    let db = Connection::open(&incoming).map_err(err)?;
    // A whole backup (`aio_files`) is all of data/; a store-only one isn't.
    let (whole, files): (bool, Vec<(String, Vec<u8>)>) =
        match db.prepare("SELECT path, bytes FROM aio_files ORDER BY path") {
            Ok(mut q) => (
                true,
                q.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?,
            ),
            Err(_) => (false, Vec::new()),
        };
    db.execute_batch("DROP TABLE aio_backup; DROP TABLE IF EXISTS aio_files").map_err(err)?;
    drop(db);
    // Never a path out of the data dir, whatever the file says.
    if let Some((bad, _)) =
        files.iter().find(|(p, _)| p.split('/').any(|c| c.is_empty() || c == "." || c == "..") || left_out(p))
    {
        _ = std::fs::remove_file(&incoming);
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{} holds a bad path: {bad}", file.display())));
    }
    crate::paths::sync_file(&incoming)?;
    let mark = format!(".replaced-{}", crate::store::now_ms());
    let aside = set_aside(store, &mark)?;
    crate::paths::rename_over(&incoming, store)?;
    // The other files: what each replaces is set aside the same way, and
    // after a whole backup so is each file it didn't hold (data/ is then
    // what was backed up, nothing newer mixed in).
    if whole {
        let mut now = Vec::new();
        self::files(crate::paths::parent(store), "", &name(store), &mut now)?;
        for path in now.iter().filter(|p| !files.iter().any(|(f, _)| f == *p)) {
            set_aside(&crate::paths::parent(store).join(path), &mark)?;
        }
    }
    for (path, bytes) in &files {
        let to = crate::paths::parent(store).join(path);
        set_aside(&to, &mark)?;
        std::fs::create_dir_all(crate::paths::parent(&to))?;
        crate::paths::write_atomic(&to, bytes, true)?;
    }
    // The good copy is of the data set aside: the next boot makes a new one.
    match std::fs::remove_file(with_suffix(store, ".good")) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    crate::paths::sync_dir(crate::paths::parent(store))?;
    Ok(aside)
}

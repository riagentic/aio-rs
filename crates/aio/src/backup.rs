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
    // `secrets`: sealed for this machine (`aio::secrets`), as the keys are.
    const OUT: [&str; 8] =
        ["instance.lock", "health.json", "serving.json", "sync.token", "paired.json", "secrets", "electron", "backups"];
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

/// How much of a file one row of a whole backup holds: SQLite takes at most
/// 1e9 bytes in one value, so a bigger file is kept in parts (`aio_parts`).
const PART: u64 = if cfg!(test) { 1000 } else { 256 << 20 };

/// `file` into the backup `db` as `rel`: a SQLite database copied
/// consistently (its WAL in), and read a part at a time, never whole.
fn put(db: &Connection, rel: &str, file: &Path, scratch: &Path) -> io::Result<()> {
    use std::io::Read;
    let mut head = [0u8; 16];
    let n = std::fs::File::open(file)?.read(&mut head)?;
    let from = if head[..n] == *b"SQLite format 3\0" {
        _ = std::fs::remove_file(scratch);
        let src = Connection::open_with_flags(file, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
        src.execute("VACUUM INTO ?1", [scratch.to_string_lossy()]).map_err(err)?;
        scratch
    } else {
        file
    };
    let put = (|| {
        let mut f = std::fs::File::open(from)?;
        for n in 0i64.. {
            let mut part = Vec::new();
            (&mut f).take(PART).read_to_end(&mut part)?;
            if n > 0 && part.is_empty() {
                break;
            }
            let (table, last) = (if n == 0 { "aio_files" } else { "aio_parts" }, (part.len() as u64) < PART);
            match n {
                0 => db.execute("INSERT INTO aio_files VALUES (?1, ?2)", rusqlite::params![rel, part]),
                _ => db.execute("INSERT INTO aio_parts VALUES (?1, ?2, ?3)", rusqlite::params![rel, n, part]),
            }
            .map_err(|e| err(format!("{rel}: {table}: {e}")))?;
            if last {
                break;
            }
        }
        Ok(())
    })();
    if from == scratch {
        _ = std::fs::remove_file(scratch);
    }
    put
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
            db.execute_batch(
                "CREATE TABLE aio_files (path TEXT PRIMARY KEY, bytes BLOB NOT NULL);
                 CREATE TABLE aio_parts (path TEXT NOT NULL, n INTEGER NOT NULL, bytes BLOB NOT NULL,
                                         PRIMARY KEY (path, n))",
            )
            .map_err(err)?;
            let scratch = with_suffix(&partial, ".db");
            for f in list {
                put(&db, &f, &data.join(&f), &scratch)?;
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

/// The timed backups in `dir`, by name (oldest first), and which of them to
/// remove so the newest `keep` stay (pure).
fn timed_names(id: &str, names: impl Iterator<Item = String>, keep: usize) -> (Vec<String>, Vec<String>) {
    let prefix = format!("{id}-auto-");
    let mut ours: Vec<String> = names.filter(|n| n.starts_with(&prefix) && n.ends_with(".aio-backup")).collect();
    ours.sort();
    let gone = ours[..ours.len().saturating_sub(keep.max(1))].to_vec();
    (ours, gone)
}

/// One timed backup of app `id` into `dir` now, and the oldest beyond `keep`
/// removed; the new file.
pub fn timed_once(id: &str, store: &Path, dir: &Path, keep: usize) -> io::Result<PathBuf> {
    crate::paths::private_dir(dir)?;
    let stamp = crate::log::stamp(crate::store::now_ms()).replace([':', '.'], "-");
    let to = dir.join(format!("{id}-auto-{stamp}.aio-backup"));
    backup_all(id, store, &to)?;
    let names = std::fs::read_dir(dir)?.flatten().map(|e| e.file_name().to_string_lossy().into_owned());
    // Never the new one, though backups dated ahead of the clock sort after it.
    for old in timed_names(id, names, keep).1.into_iter().filter(|n| to.file_name() != Some(n.as_ref())) {
        std::fs::remove_file(dir.join(old))?;
    }
    Ok(to)
}

/// Timed backups (`[data] backup_every`): app `id`'s store and data, as
/// `ax backup` makes them, into `dir` (`<home>/backups`) as
/// `<id>-auto-<time>.aio-backup` every `every`, the newest `keep` kept
/// (backups made by hand are never removed). The clock runs from the newest
/// timed backup, so restarts don't put one off; the first is made at once.
/// A failed one is said and tried again at the next turn.
pub fn timed(id: &str, store: &Path, dir: &Path, every: std::time::Duration, keep: usize) {
    let (id, store, dir) = (id.to_string(), store.to_path_buf(), dir.to_path_buf());
    let run = move || {
        loop {
            let names = std::fs::read_dir(&dir).into_iter().flatten().flatten();
            let newest = timed_names(&id, names.map(|e| e.file_name().to_string_lossy().into_owned()), keep).0.pop();
            // One dated ahead of the clock counts as new: never a backup
            // after another until the clock gets there.
            let age = newest
                .and_then(|n| std::fs::metadata(dir.join(n)).ok()?.modified().ok())
                .map_or(every, |t| t.elapsed().unwrap_or_default());
            std::thread::sleep(every.saturating_sub(age));
            if let Err(e) = timed_once(&id, &store, &dir, keep) {
                eprintln!("[aio] backup: the timed backup of {id} failed: {e}; trying again in {every:?}");
                std::thread::sleep(every);
            }
        }
    };
    if let Err(e) = std::thread::Builder::new().name("aio-backup".into()).spawn(run) {
        eprintln!("[aio] backup: no thread for timed backups: {e}");
    }
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
    let (whole, files): (bool, Vec<String>) = match db.prepare("SELECT path FROM aio_files ORDER BY path") {
        Ok(mut q) => (true, q.query_map([], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?),
        Err(_) => (false, Vec::new()),
    };
    // Never a path out of the data dir, whatever the file says.
    if let Some(bad) = files.iter().find(|p| p.split('/').any(|c| c.is_empty() || c == "." || c == "..") || left_out(p)) {
        drop(db);
        _ = std::fs::remove_file(&incoming);
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{} holds a bad path: {bad}", file.display())));
    }
    // Each file is written out of the backup a part at a time, beside where
    // it goes (`.tmp`: no backup takes it), before anything is replaced.
    let staged = with_suffix(store, ".restoring.tmp");
    _ = std::fs::remove_dir_all(&staged);
    let unpacked = (|| {
        for path in &files {
            unpack(&db, path, &staged.join(path))?;
        }
        db.execute_batch("DROP TABLE aio_backup; DROP TABLE IF EXISTS aio_files; DROP TABLE IF EXISTS aio_parts")
            .map_err(err)?;
        // The files' bytes are no part of the store.
        if whole {
            db.execute_batch("VACUUM").map_err(err)?;
        }
        Ok::<_, io::Error>(())
    })();
    drop(db);
    if let Err(e) = unpacked {
        _ = std::fs::remove_dir_all(&staged);
        _ = std::fs::remove_file(&incoming);
        return Err(e);
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
        for path in now.iter().filter(|p| !files.contains(p)) {
            set_aside(&crate::paths::parent(store).join(path), &mark)?;
        }
    }
    for path in &files {
        let to = crate::paths::parent(store).join(path);
        set_aside(&to, &mark)?;
        std::fs::create_dir_all(crate::paths::parent(&to))?;
        crate::paths::rename_over(&staged.join(path), &to)?;
        crate::paths::sync_dir(crate::paths::parent(&to))?;
    }
    _ = std::fs::remove_dir_all(&staged);
    // The good copy is of the data set aside: the next boot makes a new one.
    match std::fs::remove_file(with_suffix(store, ".good")) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    crate::paths::sync_dir(crate::paths::parent(store))?;
    Ok(aside)
}

/// File `path` of the backup `db`, its parts in order, written to `to`
/// (only this user's) and flushed.
fn unpack(db: &Connection, path: &str, to: &Path) -> io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(crate::paths::parent(to))?;
    let mut f = crate::paths::create(to, true)?;
    let first: Vec<u8> =
        db.query_row("SELECT bytes FROM aio_files WHERE path = ?1", [path], |r| r.get(0)).map_err(err)?;
    f.write_all(&first)?;
    // A backup made before files were kept in parts has no `aio_parts`.
    let parted: bool = db
        .query_row("SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'aio_parts'", [], |r| r.get(0))
        .map_err(err)?;
    if parted {
        let mut q = db.prepare("SELECT bytes FROM aio_parts WHERE path = ?1 ORDER BY n").map_err(err)?;
        let mut rows = q.query([path]).map_err(err)?;
        while let Some(r) = rows.next().map_err(err)? {
            f.write_all(&r.get::<_, Vec<u8>>(0).map_err(err)?)?;
        }
    }
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_backups_keep_the_newest_and_never_a_hand_made_one() {
        let names = ["notes-auto-2026-01-03.aio-backup", "notes-2026-01-01.aio-backup", "notes-auto-2026-01-01.aio-backup", "notes-auto-2026-01-02.aio-backup", "todo-auto-2026-01-01.aio-backup", "notes-auto-x.aio-backup.partial"];
        let (ours, gone) = timed_names("notes", names.iter().map(|n| n.to_string()), 2);
        assert_eq!(ours.len(), 3);
        assert_eq!(gone, ["notes-auto-2026-01-01.aio-backup"]);
        assert!(timed_names("notes", names.iter().map(|n| n.to_string()), 0).1.len() == 2, "at least one kept");
    }

    #[test]
    fn a_file_bigger_than_a_part_comes_back_whole() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path().join("app/data");
        let store = data.join("state.db");
        std::fs::create_dir_all(data.join("media")).unwrap();
        Connection::open(&store).unwrap().execute_batch("CREATE TABLE t (x); INSERT INTO t VALUES (1)").unwrap();
        let sizes = [0, 1, PART - 1, PART, PART + 1, 3 * PART + 7];
        let body = |n: u64| (0..n).map(|i| (i * 31 % 251) as u8).collect::<Vec<u8>>();
        for n in sizes {
            std::fs::write(data.join(format!("media/{n}.bin")), body(n)).unwrap();
        }
        let file = d.path().join("app.aio-backup");
        backup_all("app", &store, &file).unwrap();
        for n in sizes {
            std::fs::write(data.join(format!("media/{n}.bin")), "later").unwrap();
        }
        let dirs = crate::paths::AppDirs::at(&d.path().join("app"), "app").unwrap();
        restore("app", &file, &store, &dirs).unwrap();
        for n in sizes {
            assert_eq!(std::fs::read(data.join(format!("media/{n}.bin"))).unwrap(), body(n), "{n} bytes");
        }
        let db = Connection::open(&store).unwrap();
        let left: i64 = db.query_row("SELECT count(*) FROM sqlite_master WHERE name LIKE 'aio_%'", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "the backup's own tables go");
        assert!(!with_suffix(&store, ".restoring.tmp").exists());
    }

    #[test]
    fn a_timed_backup_is_a_whole_backup_and_the_oldest_go() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("data/state.db");
        std::fs::create_dir_all(store.parent().unwrap()).unwrap();
        Connection::open(&store).unwrap().execute_batch("CREATE TABLE t (x); INSERT INTO t VALUES (1)").unwrap();
        let dir = d.path().join("backups");
        let made: Vec<PathBuf> = (0..3)
            .map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(2));
                timed_once("notes", &store, &dir, 2).unwrap()
            })
            .collect();
        assert_eq!(app_of(&made[2]).unwrap(), "notes");
        assert!(!made[0].exists() && made[1].exists() && made[2].exists());
    }

    /// Backups dated ahead of the clock (it was set back, or they came from
    /// another machine): the one just made is kept though its name sorts
    /// first, and the timer waits a turn instead of making one after
    /// another (a newest backup whose age can't be told is a new one).
    #[test]
    fn backups_from_ahead_of_the_clock_never_cost_the_new_one_nor_a_loop() {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("data/state.db");
        std::fs::create_dir_all(store.parent().unwrap()).unwrap();
        Connection::open(&store).unwrap().execute_batch("CREATE TABLE t (x)").unwrap();
        let dir = d.path().join("backups");
        std::fs::create_dir_all(&dir).unwrap();
        let ahead = std::time::SystemTime::now() + std::time::Duration::from_secs(400 * 86_400);
        for n in ["notes-auto-2999-01-01T00-00-00-000Z", "notes-auto-2999-01-02T00-00-00-000Z"] {
            let f = std::fs::File::create(dir.join(format!("{n}.aio-backup"))).unwrap();
            f.set_modified(ahead).unwrap();
        }
        let made = timed_once("notes", &store, &dir, 2).unwrap();
        assert!(made.exists(), "the new backup is kept");
        let count = || std::fs::read_dir(&dir).unwrap().count();
        let before = count();
        timed("notes", &store, &dir, std::time::Duration::from_secs(3600), 2);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(count(), before, "no backup made at once");
    }
}

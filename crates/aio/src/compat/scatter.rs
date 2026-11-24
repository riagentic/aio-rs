//! One-time moves into an app's home: an app's own older layout as its
//! `[home] moved` declares ([`moved`]), and with feature `compat` aio's
//! move of a ported app's scattered layout (`app-dirs-migrate.ts`):
//! `./data.db` (+ `-wal`, `-shm`, `.journal`, `.journal.wm`) to the store,
//! `~/.local/share/<appId>/auth.db` to `data/auth.db`, `./.aio-tls/*` to
//! `data/tls/`, `./.aio/log/*` to the logs.
//! It moves what a user cannot make again, so a partial failure is always
//! survivable: never onto an existing target, a database with its WAL as
//! one set (recorded in `<to>.moving` first), a copy across filesystems
//! only takes the target's name once whole, and a WAL another database may
//! own is never applied.
use std::io;
use std::path::{Path, PathBuf};

/// How one file's move went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Moved,
    /// Its target exists: nothing was overwritten.
    Kept,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Move {
    pub from: PathBuf,
    pub to: PathBuf,
    pub outcome: Outcome,
}

/// Where a scattered app's files were: `cwd` (the folder it was started
/// in) and `xdg` (`$XDG_DATA_HOME/<appId>`, else `~/.local/share/<appId>`).
/// Where they go: `data`, its `store` and `logs`.
#[derive(Clone, Debug)]
pub struct Layout {
    pub cwd: PathBuf,
    pub xdg: PathBuf,
    pub data: PathBuf,
    pub store: PathBuf,
    pub logs: PathBuf,
}

fn with(p: &Path, s: &str) -> PathBuf {
    let mut o = p.as_os_str().to_owned();
    o.push(s);
    o.into()
}

fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// The record of a database move in progress: the only proof that a WAL
/// left at `<from>-wal` belongs to `to`.
pub fn record_of(to: &Path) -> PathBuf {
    with(to, ".moving")
}

/// Moves files with `rename` (`std::fs::rename`; a test fails it to take
/// the copy path).
struct Mover(fn(&Path, &Path) -> io::Result<()>);

impl Mover {
    /// One file: renamed, else copied as `<to>.partial`, checked, flushed
    /// and renamed into place, and only then the original removed.
    fn file(&self, from: &Path, to: &Path) -> Move {
        let done = |outcome| Move { from: from.into(), to: to.into(), outcome };
        if !exists(from) || exists(to) {
            return done(Outcome::Kept);
        }
        // An earlier death's torn copy: never valid.
        let partial = with(to, ".partial");
        if exists(&partial)
            && let Err(e) = std::fs::remove_file(&partial)
        {
            return done(Outcome::Failed(format!(
                "could not clear an interrupted copy at {} ({e}): the original is left in place",
                partial.display()
            )));
        }
        if (self.0)(from, to).is_ok() {
            return done(Outcome::Moved);
        }
        let copied = (|| {
            let src = std::fs::metadata(from)?;
            std::fs::copy(from, &partial)?;
            // The source's time, as a rename keeps it: a WAL stranded after
            // this copy is still provably older than the database.
            let f = std::fs::OpenOptions::new().write(true).open(&partial)?;
            f.set_modified(src.modified()?)?;
            if f.metadata()?.len() != src.len() {
                drop(f);
                _ = std::fs::remove_file(&partial);
                return Err(io::Error::other("copy size mismatch: the original is left in place"));
            }
            f.sync_all()?;
            crate::paths::rename_over(&partial, to)?;
            // Its name on disk before the original goes (another disk:
            // nothing orders the two).
            crate::paths::sync_dir(crate::paths::parent(to))?;
            std::fs::remove_file(from)
        })();
        match copied {
            Ok(()) => done(Outcome::Moved),
            Err(e) => {
                if !exists(to) {
                    _ = std::fs::remove_file(&partial);
                }
                done(Outcome::Failed(e.to_string()))
            }
        }
    }

    /// Drop a finished move's record; one that stays is a failed move of
    /// its own (it would keep vouching for a WAL at `from`).
    fn drop_record(&self, to: &Path) -> Vec<Move> {
        let r = record_of(to);
        match std::fs::remove_file(&r) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => vec![Move {
                from: r.clone(),
                to: r,
                outcome: Outcome::Failed(format!(
                    "could not remove the finished move's record ({e}): delete it by hand"
                )),
            }],
            _ => vec![],
        }
    }

    /// A SQLite file with its `-wal` and `-shm`, recorded before the first
    /// rename; a sidecar that did not move keeps the record for a retry.
    fn database(&self, from: &Path, to: &Path) -> Vec<Move> {
        if !exists(from) || exists(to) {
            return vec![self.file(from, to)];
        }
        let record = record_of(to);
        let text = serde_json::json!({ "from": from.to_string_lossy() }).to_string();
        if let Err(e) = crate::paths::write_atomic(&record, text.as_bytes(), true) {
            return vec![Move {
                from: from.into(),
                to: to.into(),
                outcome: Outcome::Failed(format!(
                    "could not record the move at {} ({e}): nothing was moved",
                    record.display()
                )),
            }];
        }
        let mut out = vec![self.file(from, to)];
        if out[0].outcome != Outcome::Moved {
            return [out, self.drop_record(to)].concat();
        }
        for ext in ["-wal", "-shm", "-journal"] {
            if exists(&with(from, ext)) {
                out.push(self.file(&with(from, ext), &with(to, ext)));
            }
        }
        if out.iter().all(|m| m.outcome == Outcome::Moved) {
            out.extend(self.drop_record(to));
        }
        out
    }

    /// Finish a [`Self::database`] that died between the database and its
    /// WAL. The WAL is carried over only while `to` provably is its
    /// database: the record names `from`, `to` has no WAL of its own and
    /// was not written after the WAL was. Anything else is said, and left.
    fn finish(&self, from: &Path, to: &Path) -> Vec<Move> {
        if exists(from) || !exists(to) {
            return vec![];
        }
        let recorded = std::fs::read(record_of(to))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .is_some_and(|v| v["from"].as_str() == Some(&*from.to_string_lossy()));
        let (wal, shm) = (with(from, "-wal"), with(from, "-shm"));
        if !exists(&wal) {
            if !recorded {
                return vec![];
            }
            let mut out = vec![];
            for ext in ["-shm", "-journal"] {
                if exists(&with(from, ext)) && !exists(&with(to, ext)) {
                    out.push(self.file(&with(from, ext), &with(to, ext)));
                }
            }
            out.extend(self.drop_record(to));
            return out;
        }
        let failed =
            |why: String| vec![Move { from: wal.clone(), to: with(to, "-wal"), outcome: Outcome::Failed(why) }];
        if !recorded {
            return failed(format!(
                "a WAL is left at {} with no database beside it, and nothing records that {} was moved from {}: it may belong to another database, so it was NOT applied. Keep it for a recovery tool, or delete it if that database is gone for good",
                wal.display(),
                to.display(),
                from.display()
            ));
        }
        let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let older = matches!((mtime(to), mtime(&wal)), (Some(a), Some(b)) if a <= b);
        if exists(&with(to, "-wal")) || !older {
            return failed(format!(
                "a WAL was left behind by an interrupted move, but {} has been written since: it was NOT applied (it could damage the database). It may hold writes missing from it; keep it for a recovery tool",
                to.display()
            ));
        }
        let mut out = vec![self.file(&wal, &with(to, "-wal"))];
        if out[0].outcome == Outcome::Moved && exists(&shm) {
            out.push(self.file(&shm, &with(to, "-shm")));
        }
        if out.iter().all(|m| m.outcome == Outcome::Moved) {
            out.extend(self.drop_record(to));
        }
        out
    }

    /// The journal with its `.wm` watermark, the watermark first: a
    /// journal without its watermark would replay everything again, so one
    /// whose watermark stays (its target exists, or it failed) stays too.
    fn journal(&self, from: &Path, to: &Path) -> Vec<Move> {
        if !exists(from) {
            return vec![];
        }
        let mut out = vec![];
        if exists(&with(from, ".wm")) {
            let wm = self.file(&with(from, ".wm"), &with(to, ".wm"));
            let apart = wm.outcome != Outcome::Moved;
            out.push(wm);
            if apart {
                return out;
            }
        }
        out.push(self.file(from, to));
        out
    }

    /// Every file of a folder (not its folders), then the folder if empty.
    fn dir(&self, from: &Path, to: &Path) -> Vec<Move> {
        let Ok(entries) = std::fs::read_dir(from) else { return vec![] };
        let files: Vec<PathBuf> =
            entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file())).map(|e| e.path()).collect();
        if let Err(e) = crate::paths::private_dir(to) {
            return vec![Move { from: from.into(), to: to.into(), outcome: Outcome::Failed(e.to_string()) }];
        }
        let out: Vec<Move> = files.iter().map(|f| self.file(f, &to.join(f.file_name().unwrap_or_default()))).collect();
        if out.iter().all(|m| m.outcome == Outcome::Moved) {
            _ = std::fs::remove_dir(from);
        }
        out
    }

    /// `[home] moved`: each file to its own path under `home`, as a
    /// database (with its sidecars) where one is.
    fn moved(&self, rules: &[crate::config::Moved], home: &Path) -> Vec<Move> {
        let mut out = vec![];
        for r in rules {
            let Some((var, rest)) = crate::config::moved_from(&r.from) else { continue };
            let base = if var == "HOME" { crate::paths::user_home() } else { crate::paths::xdg_data_home() };
            for f in &r.files {
                let (from, to) = (base.join(rest).join(f), home.join(f));
                if exists(&from)
                    && !exists(&to)
                    && let Err(e) = crate::paths::private_dir(crate::paths::parent(&to))
                {
                    out.push(Move { from, to, outcome: Outcome::Failed(e.to_string()) });
                    continue;
                }
                out.extend(self.finish(&from, &to));
                out.extend(self.database(&from, &to));
            }
        }
        out.into_iter().filter(|m| m.outcome != Outcome::Kept || exists(&m.from)).collect()
    }

    fn adopt(&self, l: &Layout) -> Vec<Move> {
        let (db, auth) = (l.cwd.join("data.db"), l.xdg.join("auth.db"));
        let auth_to = l.data.join("auth.db");
        // The folder is a scattered app's only while its store or journal is there
        // (or its move is under way): any aio project folder has a `.aio/`, and
        // another app's logs are never taken.
        let ours =
            [&db, &with(&db, "-wal"), &with(&db, ".journal"), &record_of(&l.store)].into_iter().any(|p| exists(p));
        let theirs = [&auth, &with(&auth, "-wal"), &record_of(&auth_to)].into_iter().any(|p| exists(p));
        let mut out = vec![];
        if ours {
            out.extend(self.finish(&db, &l.store));
        }
        if theirs {
            out.extend(self.finish(&auth, &auth_to));
        }
        if ours {
            out.extend(self.database(&db, &l.store));
            // Where the store's replay reads it (`<data>/journal` beside
            // `state.db`, `<store>.journal` beside another name).
            out.extend(self.journal(&with(&db, ".journal"), &crate::compat::data::journal_of(&l.store)));
        }
        if theirs {
            out.extend(self.database(&auth, &auth_to));
        }
        if ours {
            out.extend(self.dir(&l.cwd.join(".aio-tls"), &l.data.join("tls")));
            out.extend(self.dir(&l.cwd.join(".aio").join("log"), &l.logs));
        }
        out.into_iter()
            // A target already there is said only while its original is too.
            .filter(|m| m.outcome != Outcome::Kept || exists(&m.from))
            .collect()
    }
}

/// Move layout `l`'s scattered files into the home. Idempotent: with
/// nothing scattered left (or every target there already) it does nothing.
/// Run with the home claimed, before anything opens the store.
pub fn adopt(l: &Layout) -> Vec<Move> {
    Mover(|a, b| std::fs::rename(a, b)).adopt(l)
}

/// Move the files of an app's older layouts (`[home] moved`) into its
/// `home`, as [`adopt`] does: never over one there, a database with its
/// sidecars. `~/` and `$XDG_DATA_HOME` are a test's own world in a test.
pub fn moved(rules: &[crate::config::Moved], home: &Path) -> Vec<Move> {
    Mover(|a, b| std::fs::rename(a, b)).moved(rules, home)
}

/// What [`adopt`] did, one line a move (none when nothing was there).
pub fn said(moves: &[Move], data: &Path) -> Vec<String> {
    let moved: Vec<&Move> = moves.iter().filter(|m| m.outcome == Outcome::Moved).collect();
    let mut out = vec![];
    if !moved.is_empty() {
        out.push(format!("data: moved into {} (one folder, one backup):", crate::paths::parent(data).display()));
        out.extend(moved.iter().map(|m| format!("data:   {} → {}", m.from.display(), m.to.display())));
    }
    for m in moves {
        match &m.outcome {
            Outcome::Kept => out.push(format!(
                "data: kept {}: {} already exists (nothing was overwritten)",
                m.from.display(),
                m.to.display()
            )),
            Outcome::Failed(why) => {
                out.push(format!("data: FAILED {}: {why}", m.from.file_name().unwrap_or_default().to_string_lossy()))
            }
            Outcome::Moved => {}
        }
    }
    if !moved.is_empty() {
        out.push(format!("data: back up {}: everything outside it is disposable", data.display()));
    }
    out
}

/// The move of aio app `app`'s scattered layout (in the folder it starts in)
/// into its home, each move said. Skipped while the app runs from its
/// default home in another process: that one holds the files open.
#[cfg(feature = "compat")]
pub(crate) fn adopt_here(app: &str, dirs: &crate::paths::AppDirs) {
    let user = crate::paths::home();
    let default = user.join(format!(".{app}")).join("data");
    if default != dirs.data && crate::paths::running(&default) {
        eprintln!("[aio] data: the scattered files are not moved: {app} is running from {}", default.display());
        return;
    }
    let xdg = crate::paths::xdg_data_home();
    let Ok(cwd) = std::env::current_dir() else { return };
    let l =
        Layout { cwd, xdg: xdg.join(app), data: dirs.data.clone(), store: dirs.store.clone(), logs: dirs.logs.clone() };
    for line in said(&adopt(&l), &dirs.data) {
        eprintln!("[aio] {line}");
    }
}

/// [`moved`] into the home of `dirs`, each move said: only the app's own
/// home, never a profile's, an instance's, `--home`, `AIORT_DATA` (a test's,
/// a smoke run's) or a debug build's `[home] dev`.
pub(crate) fn moved_here(rules: &[crate::config::Moved], dirs: &crate::paths::AppDirs) {
    let own = ["default", "[home] dir", "[home] env", "[home] portable", "AIO_APPS_DIR", "AIORT_APPS_DIR"];
    if rules.is_empty() || dirs.profile.is_some() || !own.contains(&dirs.from) {
        return;
    }
    for line in said(&moved(rules, crate::paths::parent(&dirs.data)), &dirs.data) {
        eprintln!("[aio] {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    /// aio's scattered app: the store with its sidecars and journal in the
    /// project folder, auth.db in the XDG folder, certs and logs in dot dirs.
    fn scattered() -> (tempfile::TempDir, Layout) {
        let d = tempfile::tempdir().unwrap();
        let (cwd, xdg, home) = (d.path().join("project"), d.path().join("xdg/purse"), d.path().join(".purse"));
        for dir in [cwd.join(".aio-tls"), cwd.join(".aio/log"), xdg.clone()] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for (f, t) in [
            ("data.db", "STATE"),
            ("data.db-wal", "WAL"),
            ("data.db-shm", "SHM"),
            ("data.db.journal", "JOURNAL"),
            (".aio-tls/tls-cert.pem", "CERT"),
            (".aio-tls/tls-key.pem", "KEY"),
            (".aio/log/app.log", "LOG"),
        ] {
            std::fs::write(cwd.join(f), t).unwrap();
        }
        std::fs::write(xdg.join("auth.db"), "USERS").unwrap();
        let data = home.join("data");
        crate::paths::private_dir(&data).unwrap();
        let l = Layout { cwd, xdg, store: data.join("vault.db"), data, logs: home.join("logs") };
        (d, l)
    }

    #[test]
    fn every_scattered_file_lands_in_the_home_once() {
        let (_d, l) = scattered();
        let moves = adopt(&l);
        assert!(moves.iter().all(|m| m.outcome == Outcome::Moved), "{moves:?}");
        assert_eq!(moves.len(), 8);
        let s = |p: &str| with(&l.store, p);
        assert_eq!([read(&l.store), read(&s("-wal")), read(&s("-shm"))], ["STATE", "WAL", "SHM"]);
        assert_eq!(read(&crate::compat::data::journal_of(&l.store)), "JOURNAL");
        assert_eq!(read(&l.data.join("auth.db")), "USERS");
        assert_eq!(read(&l.data.join("tls/tls-key.pem")), "KEY");
        assert_eq!(read(&l.logs.join("app.log")), "LOG");
        // One copy of a database, never two; no record left; folders gone.
        for p in ["data.db", "data.db-wal", "data.db.journal", ".aio-tls", ".aio/log"] {
            assert!(!exists(&l.cwd.join(p)), "{p}");
        }
        assert!(!exists(&record_of(&l.store)) && !exists(&l.xdg.join("auth.db")));
        let lines = said(&moves, &l.data);
        assert!(lines[0].starts_with("data: moved into") && lines.last().unwrap().contains("back up"), "{lines:?}");
        // Idempotent: a second run, and a modern app, do nothing.
        assert!(adopt(&l).is_empty());
        assert!(said(&[], &l.data).is_empty());
    }

    #[test]
    fn an_existing_target_is_never_overwritten_and_sidecars_stay_with_their_database() {
        let (_d, l) = scattered();
        std::fs::write(&l.store, "NEW").unwrap();
        let moves = adopt(&l);
        assert_eq!(read(&l.store), "NEW");
        // The legacy database stays whole, with its own WAL beside it.
        assert_eq!([read(&l.cwd.join("data.db")), read(&l.cwd.join("data.db-wal"))], ["STATE", "WAL"]);
        assert!(!exists(&with(&l.store, "-wal")));
        let kept = moves.iter().find(|m| m.from == l.cwd.join("data.db")).unwrap();
        assert_eq!(kept.outcome, Outcome::Kept);
        assert!(said(&moves, &l.data).iter().any(|s| s.contains("already exists")));
    }

    #[test]
    fn the_journals_watermark_travels_with_it_first() {
        let (_d, l) = scattered();
        std::fs::write(l.cwd.join("data.db.journal.wm"), "7").unwrap();
        let j = crate::compat::data::journal_of(&l.store);
        std::fs::write(with(&j, ".wm"), "old").unwrap();
        let moves = adopt(&l);
        // The watermark's target exists: the journal is kept with it.
        assert_eq!(read(&l.cwd.join("data.db.journal")), "JOURNAL");
        assert!(!exists(&j), "{moves:?}");
        std::fs::remove_file(with(&j, ".wm")).unwrap();
        adopt(&l);
        assert_eq!([read(&j), read(&with(&j, ".wm"))], ["JOURNAL", "7"]);
    }

    #[test]
    fn the_journal_lands_where_the_stores_replay_reads_it() {
        let (_d, l) = scattered();
        std::fs::write(l.cwd.join("data.db.journal.wm"), "7").unwrap();
        adopt(&l);
        let j = crate::compat::data::journal_of(&l.store);
        assert_eq!([read(&j), read(&with(&j, ".wm"))], ["JOURNAL", "7"]);
    }

    #[test]
    fn a_move_that_died_before_its_wal_is_finished_only_under_its_record() {
        // The database moved, its WAL not: the record vouches for it.
        let (_d, l) = scattered();
        let db = l.cwd.join("data.db");
        std::fs::write(record_of(&l.store), serde_json::json!({ "from": db.to_string_lossy() }).to_string()).unwrap();
        std::fs::rename(&db, &l.store).unwrap();
        adopt(&l);
        assert_eq!([read(&l.store), read(&with(&l.store, "-wal"))], ["STATE", "WAL"]);
        assert!(!exists(&record_of(&l.store)));
        // No record: a WAL beside no database may be another's.
        let (_d, l) = scattered();
        let db = l.cwd.join("data.db");
        std::fs::rename(&db, &l.store).unwrap();
        let moves = adopt(&l);
        let f = moves.iter().find(|m| m.from == with(&db, "-wal")).unwrap();
        assert!(matches!(&f.outcome, Outcome::Failed(w) if w.contains("NOT applied")), "{f:?}");
        assert_eq!(read(&with(&db, "-wal")), "WAL");
        assert!(!exists(&with(&l.store, "-wal")));
        // Recorded, but written since (or with a WAL of its own): kept, said.
        let (_d, l) = scattered();
        let db = l.cwd.join("data.db");
        std::fs::write(record_of(&l.store), serde_json::json!({ "from": db.to_string_lossy() }).to_string()).unwrap();
        std::fs::rename(&db, &l.store).unwrap();
        std::fs::write(with(&l.store, "-wal"), "ITS OWN").unwrap();
        let moves = adopt(&l);
        assert!(moves.iter().any(|m| matches!(&m.outcome, Outcome::Failed(w) if w.contains("written since"))));
        assert_eq!(read(&with(&l.store, "-wal")), "ITS OWN");
        assert!(exists(&record_of(&l.store)), "kept for the next try");
    }

    #[test]
    fn across_filesystems_a_copy_takes_the_name_only_whole() {
        let (_d, l) = scattered();
        // A torn copy an earlier death left.
        std::fs::write(with(&l.store, ".partial"), "TORN").unwrap();
        let before = std::fs::metadata(l.cwd.join("data.db")).unwrap().modified().unwrap();
        let moves = Mover(|_, _| Err(io::Error::from(io::ErrorKind::CrossesDevices))).adopt(&l);
        assert!(moves.iter().all(|m| m.outcome == Outcome::Moved), "{moves:?}");
        assert_eq!([read(&l.store), read(&with(&l.store, "-wal"))], ["STATE", "WAL"]);
        assert!(!exists(&with(&l.store, ".partial")) && !exists(&l.cwd.join("data.db")));
        assert_eq!(std::fs::metadata(&l.store).unwrap().modified().unwrap(), before, "the source's time");
    }

    /// Across filesystems the copy's name is on disk before the original
    /// goes: a power cut between the two leaves the original, never only a
    /// `.partial` that the next start doesn't take as the store.
    #[cfg(unix)]
    #[test]
    fn across_filesystems_the_copys_name_is_flushed_before_the_original_goes() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("a.db"), d.path().join("in/a.db"));
        std::fs::create_dir_all(d.path().join("in")).unwrap();
        std::fs::write(&from, "STATE").unwrap();
        let before = crate::paths::SYNCS.get();
        let m = Mover(|_, _| Err(io::Error::from(io::ErrorKind::CrossesDevices))).file(&from, &to);
        assert_eq!((m.outcome, read(&to), exists(&from)), (Outcome::Moved, "STATE".into(), false));
        assert_eq!(crate::paths::SYNCS.get() - before, 1, "the folder it landed in, flushed");
    }

    #[test]
    fn a_folder_without_the_store_is_another_apps() {
        let (_d, l) = scattered();
        for f in ["data.db", "data.db-wal", "data.db-shm", "data.db.journal"] {
            std::fs::remove_file(l.cwd.join(f)).unwrap();
        }
        let moves = adopt(&l);
        // Only the app's own XDG auth.db comes in.
        assert_eq!(moves.iter().map(|m| m.to.clone()).collect::<Vec<_>>(), [l.data.join("auth.db")]);
        assert_eq!(read(&l.cwd.join(".aio/log/app.log")), "LOG");
        assert_eq!(read(&l.cwd.join(".aio-tls/tls-key.pem")), "KEY");
    }

    #[test]
    fn an_apps_own_older_layout_moves_in_once_never_over_a_file_and_a_database_with_its_sidecars() {
        let world = crate::paths::sandbox().unwrap();
        let name = format!("purse-{}", crate::uuid_v4().simple());
        let old = crate::paths::xdg_data_home().join(&name);
        assert!(old.starts_with(world.root()), "a test's own world");
        std::fs::create_dir_all(old.join("data")).unwrap();
        std::fs::create_dir_all(old.join("logs")).unwrap();
        for (f, t) in [
            ("data/vault.db", "VAULT"),
            ("data/vault.db-wal", "WAL"),
            ("data/vault.db-shm", "SHM"),
            ("data/vault.db-journal", "JOURNAL"),
            ("data/cache.sqlite", "CACHE"),
            ("logs/event.log", "LOG"),
        ] {
            std::fs::write(old.join(f), t).unwrap();
        }
        let files = ["data/vault.db", "data/cache.sqlite", "logs/event.log", "data/gone.db"];
        let rules = [crate::config::Moved {
            from: format!("$XDG_DATA_HOME/{name}"),
            files: files.iter().map(|f| f.to_string()).collect(),
        }];
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let moves = moved(&rules, h);
        assert!(moves.iter().all(|m| m.outcome == Outcome::Moved), "{moves:?}");
        assert_eq!(moves.len(), 6, "the missing file is not said: {moves:?}");
        let v = h.join("data/vault.db");
        assert_eq!(
            [read(&v), read(&with(&v, "-wal")), read(&with(&v, "-shm")), read(&with(&v, "-journal"))],
            ["VAULT", "WAL", "SHM", "JOURNAL"]
        );
        assert_eq!([read(&h.join("data/cache.sqlite")), read(&h.join("logs/event.log"))], ["CACHE", "LOG"]);
        assert!(!exists(&old.join("data/vault.db")) && !exists(&record_of(&v)));
        assert!(moved(&rules, h).is_empty(), "once");

        // The home has its own vault: the old one stays whole, the rest moves.
        for (f, t) in [("data/vault.db", "OLD"), ("data/vault.db-wal", "OLD WAL"), ("data/cache.sqlite", "C2")] {
            std::fs::write(old.join(f), t).unwrap();
        }
        std::fs::remove_file(h.join("data/cache.sqlite")).unwrap();
        let moves = moved(&rules, h);
        assert_eq!(read(&v), "VAULT");
        assert_eq!([read(&old.join("data/vault.db")), read(&old.join("data/vault.db-wal"))], ["OLD", "OLD WAL"]);
        assert_eq!(read(&with(&v, "-wal")), "WAL", "no orphan sidecar over its own");
        assert_eq!(moves.iter().find(|m| m.to == v).unwrap().outcome, Outcome::Kept);
        assert_eq!(read(&h.join("data/cache.sqlite")), "C2");
        assert!(said(&moves, &h.join("data")).iter().any(|l| l.contains("already exists")));

        // `~/` is the test world's home too.
        let rules = [crate::config::Moved { from: format!("~/.{name}"), files: vec!["a.db".into()] }];
        let dot = crate::paths::user_home().join(format!(".{name}"));
        std::fs::create_dir_all(&dot).unwrap();
        std::fs::write(dot.join("a.db"), "A").unwrap();
        moved(&rules, h);
        assert_eq!(read(&h.join("a.db")), "A");
    }
}

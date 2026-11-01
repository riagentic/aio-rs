use aio::prelude::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cell(persist)]
pub struct Todo {
    items: BTreeMap<Uuid, Item>,
    tags: Vec<String>,
    luck: u64,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Item {
    title: String,
    done: bool,
}

#[methods]
impl Todo {
    #[client]
    pub fn add(&mut self, cx: &Cx, title: &str) -> Result<Uuid> {
        ensure!(!title.trim().is_empty(), "empty title");
        let id = cx.id();
        self.items.insert(id, Item { title: title.into(), done: false });
        Ok(id)
    }

    pub fn toggle(&mut self, id: Uuid) -> Result {
        let Some(it) = self.items.get_mut(&id) else { reject!("no item {id}") };
        it.done = !it.done;
        Ok(())
    }

    pub fn tag(&mut self, tags: &[String]) {
        self.tags.extend_from_slice(tags);
    }

    pub fn roll(&mut self, cx: &Cx) -> Result<u64> {
        self.luck = cx.rand();
        Ok(self.luck)
    }

    /// Breaks the invariant: always refused.
    pub fn flood(&mut self) -> Result {
        self.tags = vec!["x".into(); 101];
        Ok(())
    }

    #[invariant]
    fn few_tags(&self) -> Result {
        ensure!(self.tags.len() <= 100, "too many tags");
        Ok(())
    }

    fn helper(&mut self) {}
}

/// A big state in persistent collections: each call's copy is O(1).
#[cfg(feature = "imbl")]
#[cell(persist)]
pub struct Inbox {
    mail: aio::imbl::OrdMap<u64, String>,
    order: aio::imbl::Vector<u64>,
}

#[cfg(feature = "imbl")]
#[methods]
impl Inbox {
    pub fn fill(&mut self, n: u64) {
        for i in 0..n {
            self.mail.insert(i, format!("message {i}"));
            self.order.push_back(i);
        }
    }

    pub fn put(&mut self, id: u64, text: String) {
        self.mail.insert(id, text);
        self.order.push_front(id);
    }
}

#[cfg(feature = "imbl")]
#[test]
fn persistent_collections_save_reload_and_replay_exactly() {
    let (_d, p) = tmp();
    let before = {
        let mut s = Store::<Inbox>::open(&p).unwrap();
        s.call(&By::Test, "fill", serde_json::json!([20_000])).unwrap();
        for i in 0..50u64 {
            s.call(&By::Test, "put", serde_json::json!([1_000_000 + i, format!("new {i}")])).unwrap();
        }
        s.state().clone()
    };
    let after = Store::<Inbox>::open(&p).unwrap();
    assert_eq!(*after.state(), before);
    assert_eq!((after.state().mail.len(), after.state().order.front()), (20_050, Some(&1_000_049)));
}

fn tmp() -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    (d, p)
}

/// Everything a store holds, as text (SQLite may fold its WAL into the
/// file on open, which changes bytes, not data).
fn bytes(p: &std::path::Path) -> String {
    let db = rusqlite::Connection::open_with_flags(p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut out = String::new();
    for t in ["log", "snap", "sched", "ops", "floor"] {
        let mut q = db.prepare(&format!("SELECT * FROM {t} ORDER BY 1, 2")).unwrap();
        let n = q.column_count();
        let rows = q
            .query_map([], |r| {
                (0..n)
                    .map(|i| r.get::<_, rusqlite::types::Value>(i).map(|v| format!("{v:?}")))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap();
        for r in rows {
            out += &format!("{t}: {}\n", r.unwrap().join(" | "));
        }
    }
    out
}

#[test]
fn generated_identity() {
    assert_eq!(<Todo as CellData>::ID, "todo");
    assert_eq!(<Todo as CellData>::VERSION, 1);
    assert_eq!((<Todo as CellData>::PERSIST, <Todo as CellData>::SHARED), (true, false));
    assert_eq!(<Todo as Cell>::METHODS, ["add", "toggle", "tag", "roll", "flood"]);
    assert_eq!(<Todo as Cell>::CLIENT, ["add"]);
    let mut t = Todo::default();
    t.helper();
}

#[test]
fn typed_calls_and_refusals() {
    let h = Handle::<Todo>::memory();
    let id = h.add("milk").unwrap();
    h.toggle(id).unwrap();
    assert!(h.state().items[&id].done);
    assert_eq!(h.add("  "), Err(Rejected::new("empty title")));
    assert!(h.toggle(Uuid::nil()).unwrap_err().why.starts_with("no item"));
    h.tag(&["a".into()]).unwrap();
    assert_eq!(h.state().tags, ["a"]);
    // A refused call changes nothing and logs nothing.
    let n = h.store(|s| s.events().unwrap().len());
    assert_eq!(h.flood(), Err(Rejected::new("too many tags")));
    assert_eq!(h.state().tags, ["a"]);
    assert_eq!(h.store(|s| s.events().unwrap().len()), n);
}

#[test]
fn calls_by_name_check_names_and_args() {
    let h = Handle::<Todo>::memory();
    assert_eq!(h.call("nope", serde_json::Value::Null), Err(Rejected::unknown("nope")));
    assert!(h.call("add", serde_json::json!([1])).unwrap_err().why.starts_with("bad arguments"));
    assert!(h.call("helper", serde_json::Value::Null).is_err());
}

#[test]
fn survives_reopen_with_recorded_inputs() {
    let (_d, p) = tmp();
    let (ids, luck) = {
        let h = Handle::<Todo>::open(&p).unwrap();
        let ids: Vec<_> = (0..300).map(|i| h.add(&format!("t{i}")).unwrap()).collect();
        h.toggle(ids[7]).unwrap();
        (ids, h.roll().unwrap())
    };
    let h = Handle::<Todo>::open(&p).unwrap();
    let st = h.state();
    assert_eq!(st.items.len(), 300);
    assert!(st.items[&ids[7]].done);
    assert_eq!(st.luck, luck);
    assert_eq!(h.store(|s| s.seq()), 302);
}

#[test]
fn replays_the_log_after_a_crash() {
    let (_d, p) = tmp();
    let h = Handle::<Todo>::open(&p).unwrap();
    let id = h.add("a").unwrap();
    h.roll().unwrap();
    let want = h.state();
    // A crash: no snapshot at close, only the log.
    std::mem::forget(h);
    let h = Handle::<Todo>::open(&p).unwrap();
    assert_eq!(*h.state(), *want);
    assert!(h.state().items.contains_key(&id));
}

/// Records who called it, as the method sees them.
#[cell(persist)]
pub struct Guestbook {
    signed: Vec<By>,
}

#[methods]
impl Guestbook {
    pub fn sign(&mut self, cx: &Cx) {
        self.signed.push(cx.by().clone());
    }
}

#[test]
fn methods_see_their_caller_and_replay_sees_the_same() {
    let (_d, p) = tmp();
    let h = Handle::<Guestbook>::open(&p).unwrap();
    h.by(By::User("ann".into())).sign().unwrap();
    h.sign().unwrap();
    let want = vec![By::User("ann".into()), By::System];
    assert_eq!(h.state().signed, want);
    // A crash: the log replays with each event's own caller.
    std::mem::forget(h);
    assert_eq!(Handle::<Guestbook>::open(&p).unwrap().state().signed, want);
}

#[test]
fn time_travel_and_rewind() {
    let (_d, p) = tmp();
    let h = Handle::<Todo>::open(&p).unwrap();
    let a = h.add("a").unwrap();
    let after_a = h.state();
    for i in 0..600 {
        h.add(&format!("x{i}")).unwrap();
    }
    h.toggle(a).unwrap();
    h.store(|s| {
        assert_eq!(s.state_at(0).unwrap(), Todo::default());
        assert_eq!(s.state_at(1).unwrap(), *after_a);
        assert_eq!(s.state_at(500).unwrap().items.len(), 500);
        assert!(s.state_at(10_000).is_err());
        s.rewind(&By::User("u1".into()), 1).unwrap();
    });
    assert_eq!(*h.state(), *after_a);
    drop(h);
    let h = Handle::<Todo>::open(&p).unwrap();
    assert_eq!(*h.state(), *after_a);
    let ev = h.store(|s| s.events().unwrap());
    assert_eq!(ev.last().unwrap().method, "$rewind");
    assert_eq!(ev.last().unwrap().by, By::User("u1".into()));
    // The steps in between are still there.
    assert_eq!(h.store(|s| s.state_at(601).unwrap().items.len()), 601);
}

#[test]
fn folding_keeps_the_present_and_says_what_is_gone() {
    let (_d, p) = tmp();
    let h = Handle::<Todo>::open(&p).unwrap();
    for i in 0..10 {
        h.add(&format!("t{i}")).unwrap();
    }
    let all = h.state();
    let folded = h.store(|s| s.fold_before(i64::MAX).unwrap());
    assert_eq!(folded, 10);
    h.add("after").unwrap();
    h.store(|s| {
        assert_eq!(s.state_at(10).unwrap(), *all);
        assert!(s.state_at(5).unwrap_err().0.contains("folded"));
        assert_eq!(s.events().unwrap().len(), 1);
    });
    drop(h);
    assert_eq!(Handle::<Todo>::open(&p).unwrap().state().items.len(), 11);
}

#[test]
fn history_keep_folds_on_open_and_as_commits_go_on() {
    let (_d, p) = tmp();
    let h = Handle::<Todo>::open(&p).unwrap();
    for i in 0..10 {
        h.add(&format!("t{i}")).unwrap();
    }
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert_eq!(h.store(|s| s.keep_history(std::time::Duration::from_millis(20)).unwrap()), 10, "folded on open");
    assert!(h.store(|s| s.events().unwrap().is_empty()));
    h.store(|s| s.keep_history(std::time::Duration::from_millis(50)).unwrap());
    h.add("old").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(60));
    for i in 0..300 {
        h.add(&format!("n{i}")).unwrap();
    }
    h.store(|s| {
        assert!(s.state_at(10).unwrap_err().0.contains("folded"), "folded as commits went on");
        assert!(s.events().unwrap().len() < 301);
    });
    assert_eq!(h.state().items.len(), 311, "the state stays");
}

#[test]
fn provenance_and_listeners() {
    let h = Handle::<Todo>::memory();
    let seen = std::sync::Arc::new(AtomicUsize::new(0));
    let s = seen.clone();
    h.listen(move |st| s.store(st.items.len(), Ordering::SeqCst));
    h.by(By::Agent("bot".into())).add("a").unwrap();
    h.add("b").unwrap();
    assert_eq!(seen.load(Ordering::SeqCst), 2);
    let ev = h.store(|s| s.events().unwrap());
    assert_eq!(ev[0].by, By::Agent("bot".into()));
    assert_eq!(ev[1].by, By::System);
    assert_eq!(ev[0].inputs.ids.len(), 1);
}

#[test]
fn owned_resources_drop_when_replaced() {
    struct Flag(std::sync::Arc<AtomicUsize>);
    impl Drop for Flag {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let n = std::sync::Arc::new(AtomicUsize::new(0));
    let h = Handle::<Todo>::memory();
    h.own("w", Some(Box::new(Flag(n.clone()))));
    h.own("w", Some(Box::new(Flag(n.clone()))));
    assert_eq!(n.load(Ordering::SeqCst), 1);
    h.own("w", None);
    assert_eq!(n.load(Ordering::SeqCst), 2);
    assert!(!h.owns("w"));
}

mod upgrade {
    use aio::prelude::*;

    pub mod v1 {
        use aio::prelude::*;
        #[cell(persist, id = "notes")]
        pub struct Notes {
            pub text: String,
        }
        #[methods]
        impl Notes {
            pub fn set(&mut self, t: String) {
                self.text = t;
            }
        }
    }

    fn up(old: serde_json::Value, from: u32) -> std::result::Result<Notes, String> {
        assert_eq!(from, 1);
        let text = old["text"].as_str().ok_or("no text")?;
        Ok(Notes { lines: text.lines().map(String::from).collect() })
    }

    #[cell(persist, id = "notes", version = 2, migrate = up)]
    pub struct Notes {
        pub lines: Vec<String>,
    }
    #[methods]
    impl Notes {
        pub fn push(&mut self, l: String) {
            self.lines.push(l);
        }
        /// v1's call, kept: its old events still replay.
        pub fn set(&mut self, t: String) {
            self.lines = t.lines().map(String::from).collect();
        }
    }

    pub mod bad {
        use aio::prelude::*;
        fn fail(_: serde_json::Value, _: u32) -> std::result::Result<Notes, String> {
            Err("nope".into())
        }
        #[cell(persist, id = "notes", version = 3, migrate = fail)]
        pub struct Notes {
            pub n: u32,
        }
        #[methods]
        impl Notes {}
    }

    #[test]
    fn migrates_with_a_backup_and_an_older_build_boots_on_newer_data() {
        let (_d, p) = super::tmp();
        use v1::NotesCalls as _;
        let old = Handle::<v1::Notes>::open(&p).unwrap();
        old.set("a".into()).unwrap();
        old.set("a\nb".into()).unwrap();
        drop(old);
        // A failing migration refuses to boot and leaves the file as it was.
        let before = super::bytes(&p);
        assert!(Store::<bad::Notes>::open(&p).err().unwrap().0.contains("migrate from v1 failed"));
        assert_eq!(super::bytes(&p), before);
        let h = Handle::<Notes>::open(&p).unwrap();
        assert_eq!(h.state().lines, ["a", "b"]);
        h.push("c".into()).unwrap();
        drop(h);
        assert!(
            std::fs::read_dir(p.parent().unwrap()).unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".bak"))
        );
        assert_eq!(Handle::<Notes>::open(&p).unwrap().state().lines, ["a", "b", "c"]);
        // Time travel stops at the upgrade.
        Handle::<Notes>::open(&p).unwrap().store(|s| {
            assert_eq!(s.state_at(2).unwrap().lines, ["a", "b"]);
            assert!(s.state_at(1).unwrap_err().0.contains("before the upgrade"));
        });
        // An older build boots on newer data (aio's downgrade): its writes keep
        // what it doesn't declare, and the stored version only goes up.
        let mut old = Store::<v1::Notes>::open(&p).unwrap();
        old.call(&By::Test, "set", serde_json::json!(["t"])).unwrap();
        drop(old);
        // Rolled forward (new -> old -> new): it boots as aio does, keeping
        // its own fields and dropping the old build's `text`.
        let h = Handle::<Notes>::open(&p).unwrap();
        assert_eq!(h.state().lines, ["a", "b", "c"]);
        h.push("d".into()).unwrap();
        drop(h);
        assert_eq!(Handle::<Notes>::open(&p).unwrap().state().lines, ["a", "b", "c", "d"]);
    }

    #[test]
    fn events_an_old_build_left_unsnapped_replay_after_the_upgrade() {
        let (_d, p) = super::tmp();
        use v1::NotesCalls as _;
        let old = Handle::<v1::Notes>::open(&p).unwrap();
        old.set("a".into()).unwrap();
        // The old build crashes: no snapshot of its last events.
        old.set("x\ny".into()).unwrap();
        std::mem::forget(old);
        let h = Handle::<Notes>::open(&p).unwrap();
        assert_eq!(h.state().lines, ["x", "y"]);
        drop(h);
        // An old event the new code can't run refuses the boot, data kept.
        let (_d2, p2) = super::tmp();
        let old = Handle::<v1::Notes>::open(&p2).unwrap();
        old.set("a".into()).unwrap();
        std::mem::forget(old);
        rusqlite::Connection::open(&p2).unwrap().execute("INSERT INTO log (cell, method, args, inputs, by, at, v) VALUES ('notes', 'retired', '[]', '{}', '{\"kind\":\"system\"}', 0, 1)", []).unwrap();
        let before = super::bytes(&p2);
        let e = Store::<Notes>::open(&p2).err().unwrap().0;
        assert!(e.contains("written by v1, it doesn't apply to v2"), "{e}");
        assert_eq!(super::bytes(&p2), before);
    }

    pub mod slim {
        use aio::prelude::*;
        /// v2 of "notes" with its field renamed and no version bump: a drift.
        #[cell(persist, id = "notes", version = 2)]
        pub struct Notes {
            pub rows: Vec<String>,
        }
        #[methods]
        impl Notes {}
    }

    #[test]
    fn undeclared_saved_data_is_never_dropped() {
        let (_d, p) = super::tmp();
        let h = Handle::<Notes>::open(&p).unwrap();
        h.push("keep me".into()).unwrap();
        drop(h);
        let before = super::bytes(&p);
        let e = Store::<slim::Notes>::open(&p).err().unwrap().0;
        assert!(e.contains("doesn't declare (lines)"), "{e}");
        assert_eq!(super::bytes(&p), before);
        assert_eq!(Handle::<Notes>::open(&p).unwrap().state().lines, ["keep me"]);
    }

    #[test]
    fn an_event_that_no_longer_replays_refuses_the_boot() {
        let (_d, p) = super::tmp();
        let h = Handle::<Notes>::open(&p).unwrap();
        h.push("a".into()).unwrap();
        drop(h);
        rusqlite::Connection::open(&p).unwrap().execute("INSERT INTO log (cell, method, args, inputs, by, at, v) VALUES ('notes', 'gone', '[]', '{}', '{\"kind\":\"system\"}', 0, 2)", []).unwrap();
        let before = super::bytes(&p);
        assert!(Store::<Notes>::open(&p).err().unwrap().0.contains("replay of event"));
        assert_eq!(super::bytes(&p), before);
    }
}

#[test]
fn a_foreign_store_file_is_refused_and_left_alone() {
    let (_d, p) = tmp();
    let other = rusqlite::Connection::open(&p).unwrap();
    other.execute_batch("CREATE TABLE snap (cell TEXT, version INTEGER, state TEXT)").unwrap();
    drop(other);
    let before = std::fs::read(&p).unwrap();
    assert!(Store::<Todo>::open(&p).err().unwrap().0.contains("not an aio-rs store"));
    assert_eq!(std::fs::read(&p).unwrap(), before);
}

#[test]
fn local_file_urls_are_escaped() {
    assert_eq!(aio::ui::local_file_url("/a b/ü#?.png"), "aio://app/file/a%20b/%C3%BC%23%3F.png");
}

#[test]
fn app_dirs_are_private_and_ids_are_checked() {
    let d = tempfile::tempdir().unwrap();
    // SAFETY: this test binary's only env change; no other test reads AIORT_DATA.
    // The caller's sandbox comes back after: never the real data dir.
    let caller = std::env::var_os("AIORT_DATA");
    unsafe { std::env::set_var("AIORT_DATA", d.path()) };
    let dirs = aio::paths::app("todo").unwrap();
    assert_eq!(dirs.data, d.path().join("data"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&dirs.data).unwrap().permissions().mode() & 0o777, 0o700);
    }
    // One instance: a second open is refused until the first is dropped.
    assert_eq!(aio::paths::app("todo").unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    drop(dirs);
    let dirs2 = aio::paths::app("todo").unwrap();
    // Sockets get a short private dir, the same for the same data dir.
    let s = aio::paths::sockets("todo", &dirs2.data).unwrap();
    assert_eq!(s, aio::paths::sockets("todo", &dirs2.data).unwrap());
    assert!(s.join("hand.sock").to_string_lossy().len() < 100, "{}", s.display());
    std::fs::remove_dir(&s).unwrap();
    for bad in ["", "../x", ".hidden", "a/b"] {
        assert!(aio::paths::app(bad).is_err(), "{bad}");
    }
    match caller {
        Some(v) => unsafe { std::env::set_var("AIORT_DATA", v) },
        None => unsafe { std::env::remove_var("AIORT_DATA") },
    }
}

#[test]
fn a_store_deleted_or_replaced_while_running_refuses_writes() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    let h = Handle::<Todo>::open(&p).unwrap();
    h.add("kept").unwrap();
    // Windows refuses to delete or replace a store that is open: nothing to lose.
    if cfg!(windows) {
        assert!(std::fs::remove_file(&p).is_err());
        h.add("kept too").unwrap();
        return;
    }
    // Deleted: SQLite would keep writing to the unlinked file, then lose it all.
    for f in ["state.db", "state.db-wal", "state.db-shm"] {
        _ = std::fs::remove_file(d.path().join(f));
    }
    let e = h.add("lost?").unwrap_err();
    assert!(e.why.contains("deleted or replaced"), "{e:?}");
    // Replaced by another file at the same path: refused too.
    let p2 = d.path().join("other.db");
    let h2 = Handle::<Todo>::open(&p2).unwrap();
    h2.add("one").unwrap();
    std::fs::copy(&p2, d.path().join("swap.db")).unwrap();
    std::fs::rename(d.path().join("swap.db"), &p2).unwrap();
    assert!(h2.add("two").unwrap_err().why.contains("deleted or replaced"));
}

#[test]
fn a_call_returns_only_once_its_event_is_in_the_log_file() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    let h = Handle::<Todo>::open(&p).unwrap();
    assert_eq!(h.store(|s| s.durability()).unwrap(), aio::config::Durability::Normal);
    h.store(|s| s.set_durability(aio::config::Durability::Full)).unwrap();
    assert_eq!(h.store(|s| s.durability()).unwrap(), aio::config::Durability::Full);
    let id = h.add("saved").unwrap();
    // Another reader of the file (as after a crash) sees it already.
    let other = rusqlite::Connection::open_with_flags(&p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let args: String = other.query_row("SELECT args FROM log ORDER BY seq DESC LIMIT 1", [], |r| r.get(0)).unwrap();
    assert!(args.contains("saved"), "{args}");
    drop(h);
    assert!(Handle::<Todo>::open(&p).unwrap().state().items.contains_key(&id));
}

/// aio's `dbPragmas` (`[data] pragmas`): set on the store's file, checked.
#[test]
fn the_apps_store_pragmas_are_set_and_one_sqlite_ignores_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    let h = Handle::<Todo>::open(&p).unwrap();
    h.store(|s| s.set_pragmas(&["PRAGMA application_id = 7".into()])).unwrap();
    let other = rusqlite::Connection::open(&p).unwrap();
    assert_eq!(other.query_row("PRAGMA application_id", [], |r| r.get::<_, i64>(0)).unwrap(), 7);
    let e = h.store(|s| s.set_pragmas(&["PRAGMA no_such_thing = 1".into()])).unwrap_err().0;
    assert!(e.contains("SQLite has no pragma \"no_such_thing\""), "{e}");
    let e = h.store(|s| s.set_pragmas(&["PRAGMA temp_store = PURPLE".into()])).unwrap_err().0;
    assert!(e.contains("temp_store"), "{e}");
    h.add("still works").unwrap();
}

#[test]
fn files_are_replaced_whole_and_private_ones_stay_private() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("f.json");
    aio::paths::write_atomic(&p, b"one", true).unwrap();
    aio::paths::write_atomic(&p, b"two", true).unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"two");
    // No temp files left behind.
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // A target that can't be written leaves no temp file either.
    assert!(aio::paths::write_atomic(&d.path().join("missing/f"), b"x", false).is_err());
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
}

#[test]
fn a_damaged_store_is_set_aside_and_the_good_copy_boots() {
    let (d, p) = tmp();
    let h = Handle::<Todo>::open(&p).unwrap();
    h.add("in the good copy").unwrap();
    drop(h);
    // The next boot's good copy has it; what comes after is newer than the copy.
    std::fs::remove_file(d.path().join("state.db.good")).unwrap();
    let h = Handle::<Todo>::open(&p).unwrap();
    assert!(h.store(|s| s.recovered().is_none()));
    h.add("after the copy").unwrap();
    drop(h);
    // Damage every page after the first.
    let mut b = std::fs::read(&p).unwrap();
    for x in &mut b[4096..] {
        *x = 0xAB;
    }
    std::fs::write(&p, &b).unwrap();
    let heard = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let into = heard.clone();
    aio::on_recovered(move |cell, r| into.lock().unwrap().push((cell.to_string(), r.from)));
    let h = Handle::<Todo>::open(&p).unwrap();
    let r = h.store(|s| s.recovered().cloned()).unwrap();
    assert!(heard.lock().unwrap().contains(&("todo".to_string(), r.from)), "aio's onCheckpointRestore heard it");
    let titles: Vec<String> = h.state().items.values().map(|i| i.title.clone()).collect();
    assert_eq!(titles, ["in the good copy"]);
    // Kept, not deleted.
    assert_eq!(std::fs::read(&r.quarantined).unwrap(), b);
    assert!(r.from > 0);
    drop(h);
    // Damaged with no good copy: refused, and the file is left as is.
    let (_d2, p2) = tmp();
    drop(Handle::<Todo>::open(&p2).unwrap());
    std::fs::remove_file(p2.with_extension("db.good")).unwrap();
    let mut b = std::fs::read(&p2).unwrap();
    for x in &mut b[4096..] {
        *x = 0xAB;
    }
    std::fs::write(&p2, &b).unwrap();
    assert!(Store::<Todo>::open(&p2).err().unwrap().0.contains("no good copy"));
    assert_eq!(std::fs::read(&p2).unwrap(), b);
}

#[test]
fn foreign_writes_are_refused_and_crash_leftovers_swept() {
    let (d, p) = tmp();
    let h = Handle::<Todo>::open(&p).unwrap();
    h.add("mine").unwrap();
    // Another process appends to the log behind this one's back.
    rusqlite::Connection::open(&p).unwrap().execute("INSERT INTO log (cell, method, args, inputs, by, at, v) VALUES ('todo', 'tag', '[[\"theirs\"]]', '{}', '{\"kind\":\"system\"}', 0, 1)", []).unwrap();
    assert!(h.add("lost?").unwrap_err().why.contains("something else wrote"));
    drop(h);
    // Leftovers of a crash beside the store are removed at the next open.
    for f in ["state.db.good.tmp", "state.db.restoring", ".paired.json.0123.tmp"] {
        std::fs::write(d.path().join(f), "x").unwrap();
    }
    let crashed = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
    std::fs::File::options()
        .write(true)
        .open(d.path().join(".paired.json.0123.tmp"))
        .unwrap()
        .set_modified(crashed)
        .unwrap();
    // A fresh one is another thread's, mid-write: it stays.
    std::fs::write(d.path().join(".health.json.4567.tmp"), "x").unwrap();
    std::fs::write(d.path().join("notes.txt"), "the user's").unwrap();
    let h = Handle::<Todo>::open(&p).unwrap();
    // Both writes are there after a fresh open: nothing was forked.
    assert_eq!(h.state().items.len(), 1);
    assert_eq!(h.store(|s| s.seq()), 2);
    let left: Vec<String> = std::fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|f| f.contains("tmp") || f.contains("restoring"))
        .collect();
    assert_eq!(left, [".health.json.4567.tmp"]);
    assert!(d.path().join("notes.txt").exists());
}

#[test]
fn backups_are_consistent_and_restores_keep_what_they_replace() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("app");
    let store = root.join("data/state.db");
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    let h = Handle::<Todo>::open(&store).unwrap();
    h.add("before").unwrap();
    // Made while the app runs.
    let file = d.path().join("todo.aio-backup");
    aio::backup::backup("todo", &store, &file).unwrap();
    assert!(aio::backup::backup("todo", &store, &file).is_err(), "never over an old backup");
    assert!(!d.path().join("todo.aio-backup.partial").exists());
    assert_eq!(aio::backup::app_of(&file).unwrap(), "todo");
    h.add("after").unwrap();
    drop(h);
    let data = root.join("data");
    assert!(!aio::paths::running(&data));
    let dirs = aio::paths::AppDirs::at(&root, "todo").unwrap();
    assert!(aio::paths::running(&data), "the lock says it runs");
    // A running app (its lock held) can't be restored under.
    assert_eq!(aio::paths::AppDirs::at(&root, "todo").err().unwrap().kind(), std::io::ErrorKind::WouldBlock);
    // Another app's backup is refused.
    assert!(aio::backup::restore("notes", &file, &store, &dirs).unwrap_err().to_string().contains("backup of todo"));
    let aside = aio::backup::restore("todo", &file, &store, &dirs).unwrap().unwrap();
    drop(dirs);
    assert!(!aio::paths::running(&data));
    assert_eq!(Handle::<Todo>::open(&store).unwrap().state().items.len(), 1);
    // What it replaced is kept, and still opens.
    assert_eq!(Store::<Todo>::open(&aside).unwrap().state().items.len(), 2);
    // The backup itself is unchanged: it restores again.
    assert_eq!(aio::backup::app_of(&file).unwrap(), "todo");
}

#[test]
fn a_whole_backup_holds_the_app_files_but_not_its_keys() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("app");
    let data = root.join("data");
    let store = data.join("state.db");
    std::fs::create_dir_all(data.join("reports")).unwrap();
    let h = Handle::<Todo>::open(&store).unwrap();
    h.add("one").unwrap();
    std::fs::write(data.join("reports/r1.json"), "{}").unwrap();
    std::fs::write(data.join("copy.json"), "v1").unwrap();
    std::fs::write(data.join("sync.token"), "secret").unwrap();
    std::fs::write(data.join("health.json"), "{}").unwrap();
    // Another database, open in WAL with a row only in its log.
    let auth = rusqlite::Connection::open(data.join("auth.db")).unwrap();
    auth.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE u (n TEXT); INSERT INTO u VALUES ('ana');").unwrap();
    let file = d.path().join("todo.aio-backup");
    aio::backup::backup_all("todo", &store, &file).unwrap();
    drop(auth);
    drop(h);
    // Later: the files change, the key stays this machine's.
    std::fs::write(data.join("copy.json"), "v2").unwrap();
    std::fs::remove_file(data.join("reports/r1.json")).unwrap();
    std::fs::write(data.join("sync.token"), "newer").unwrap();
    std::fs::write(data.join("reports/later.json"), "after the backup").unwrap();
    let dirs = aio::paths::AppDirs::at(&root, "todo").unwrap();
    aio::backup::restore("todo", &file, &store, &dirs).unwrap().unwrap();
    drop(dirs);
    // data/ is what was backed up: a newer file is set aside, not mixed in.
    assert!(!data.join("reports/later.json").exists());
    let later: Vec<_> = std::fs::read_dir(data.join("reports"))
        .unwrap()
        .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("later.json.replaced-"))
        .collect();
    assert_eq!(later.len(), 1);
    assert_eq!(std::fs::read_to_string(data.join("copy.json")).unwrap(), "v1");
    assert!(data.join("reports/r1.json").exists());
    assert_eq!(std::fs::read_to_string(data.join("sync.token")).unwrap(), "newer", "keys stay on their machine");
    let n: String = rusqlite::Connection::open(data.join("auth.db"))
        .unwrap()
        .query_row("SELECT n FROM u", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, "ana");
    // What it replaced is kept beside it.
    let kept: Vec<String> = std::fs::read_dir(&data)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("copy.json.replaced-"))
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(std::fs::read_to_string(data.join(&kept[0])).unwrap(), "v2");
    // A file that names a path out of the data dir, or a key, restores nothing.
    for bad in ["../evil", "reports/../../evil", "sync.token"] {
        let crafted = d.path().join("crafted.aio-backup");
        std::fs::copy(&file, &crafted).unwrap();
        let db = rusqlite::Connection::open(&crafted).unwrap();
        db.execute("INSERT INTO aio_files VALUES (?1, x'00')", [bad]).unwrap();
        drop(db);
        let dirs = aio::paths::AppDirs::at(&root, "todo").unwrap();
        let e = aio::backup::restore("todo", &crafted, &store, &dirs).unwrap_err();
        assert!(e.to_string().contains("holds a bad path"), "{e}");
        std::fs::remove_file(&crafted).unwrap();
    }
    assert!(!d.path().join("evil").exists());
    assert!(aio::backup::left_out("electron/Cache/x") && aio::backup::left_out("state.db-wal"));
    assert!(aio::backup::left_out("reports/.r2.json.9.tmp") && !aio::backup::left_out("reports/r1.json"));
}

/// Run as a child by `a_dead_holder_frees_the_lock`: hold the lock, then wait to be killed.
#[test]
fn lock_holder() {
    let Some(root) = std::env::var_os("AIO_TEST_HOLD") else { return };
    let _dirs = aio::paths::AppDirs::at(std::path::Path::new(&root), "held").unwrap();
    println!("holding");
    std::thread::sleep(std::time::Duration::from_secs(60));
}

#[cfg(unix)]
#[test]
fn a_dead_holder_frees_the_lock_even_as_a_zombie() {
    use std::io::BufRead;
    let d = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_holder", "--nocapture", "--test-threads=1"])
        .env("AIO_TEST_HOLD", d.path())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    while !line.contains("holding") {
        line.clear();
        assert!(out.read_line(&mut line).unwrap() > 0, "the holder ended early");
    }
    assert_eq!(aio::paths::AppDirs::at(d.path(), "held").err().unwrap().kind(), std::io::ErrorKind::WouldBlock);
    child.kill().unwrap();
    // Not reaped yet (a zombie): the kernel has already let the lock go.
    let mut free = false;
    for _ in 0..200 {
        if aio::paths::AppDirs::at(d.path(), "held").is_ok() {
            free = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(free, "the lock outlived its holder");
    child.wait().unwrap();
}

/// A child forked while the lock was held (any spawn, before its exec)
/// shares it: dropped here, the app is no longer running all the same.
#[cfg(unix)]
#[test]
fn a_dropped_lock_is_free_while_a_child_forked_under_it_starts() {
    use std::os::unix::process::CommandExt;
    let d = tempfile::tempdir().unwrap();
    let dirs = aio::paths::AppDirs::at(d.path(), "held").unwrap();
    let mut cmd = std::process::Command::new("true");
    // SAFETY: the forked child only sleeps before its exec.
    unsafe {
        cmd.pre_exec(|| {
            std::thread::sleep(std::time::Duration::from_millis(800));
            Ok(())
        })
    };
    let spawning = std::thread::spawn(move || cmd.status());
    std::thread::sleep(std::time::Duration::from_millis(200));
    drop(dirs);
    assert!(!aio::paths::running(&d.path().join("data")), "the forked child kept the lock");
    assert!(spawning.join().unwrap().unwrap().success());
}

/// Run as a child by `a_child_stops_when_its_parent_goes`.
#[test]
fn watched_child() {
    if std::env::var_os("AIO_TEST_WATCHED").is_none() {
        return;
    }
    aio::lifecycle::watch_parent();
    println!("watching");
    std::thread::sleep(std::time::Duration::from_secs(60));
}

#[cfg(unix)]
#[test]
fn a_child_stops_when_its_parent_goes() {
    use std::io::BufRead;
    use std::os::unix::process::ExitStatusExt;
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "watched_child", "--nocapture", "--test-threads=1"])
        .env("AIO_TEST_WATCHED", "1")
        .stdout(std::process::Stdio::piped());
    let watch = aio::lifecycle::watched(&mut cmd).unwrap();
    let mut child = cmd.spawn().unwrap();
    let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    while !line.contains("watching") {
        line.clear();
        assert!(out.read_line(&mut line).unwrap() > 0, "the child ended early");
    }
    // The parent's end closes (as when the parent dies): the child gets SIGTERM.
    drop(watch);
    let t = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(t.elapsed() < std::time::Duration::from_secs(5), "the child outlived its parent");
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(status.signal(), Some(15), "stopped by SIGTERM");
    // `stop`: a graceful ask first; a child that dies on it is reaped at once.
    let mut c = std::process::Command::new("sleep").arg("30").spawn().unwrap();
    let t = std::time::Instant::now();
    aio::lifecycle::stop(&mut c, std::time::Duration::from_secs(30));
    assert!(t.elapsed() < std::time::Duration::from_secs(10), "reaped at once, not at the grace's end");
}

#[cfg(unix)]
#[test]
fn a_planted_symlink_is_never_used_as_a_private_dir() {
    let d = tempfile::tempdir().unwrap();
    let target = d.path().join("elsewhere");
    std::fs::create_dir(&target).unwrap();
    let planted = d.path().join("aio-rt");
    std::os::unix::fs::symlink(&target, &planted).unwrap();
    let e = aio::paths::private_dir(&planted).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
    aio::paths::private_dir(&d.path().join("real")).unwrap();
}

/// What is saved of a cell (aio's `persist` filter, `onPersist`, `onRestore`):
/// the same state comes back whether the app stopped cleanly or crashed,
/// and the log still replays exactly.
mod saved {
    use super::{bytes, tmp};
    use aio::prelude::*;

    /// `cache` and `busy` live in memory only.
    #[cell(persist, id = "w")]
    pub struct Work {
        pub data: u64,
        pub done: u64,
        #[transient]
        pub cache: String,
        #[transient]
        pub busy: bool,
    }

    #[methods]
    impl Work {
        pub fn set(&mut self, v: u64) {
            self.data = v;
            self.cache = format!("CACHED-{}", v * 7);
        }
        pub fn begin(&mut self) -> Result {
            ensure!(!self.busy, "already busy");
            self.busy = true;
            Ok(())
        }
        /// Reads a transient field: replay must give it the value it had.
        pub fn finish(&mut self) -> Result {
            ensure!(self.busy, "not busy");
            self.busy = false;
            self.done += 1;
            Ok(())
        }
    }

    #[test]
    fn transient_fields_never_reach_disk_and_come_back_as_defaults() {
        assert_eq!(<Work as CellData>::TRANSIENT, ["cache", "busy"]);
        for crash in [false, true] {
            let (_d, p) = tmp();
            let h = Handle::<Work>::open(&p).unwrap();
            h.set(4242).unwrap();
            h.begin().unwrap();
            assert_eq!((h.state().cache.as_str(), h.state().busy), ("CACHED-29694", true), "live state keeps it");
            if crash {
                std::mem::forget(h);
            } else {
                drop(h);
            }
            assert!(!bytes(&p).contains("CACHED"), "crash {crash}: off disk");
            let h = Handle::<Work>::open(&p).unwrap();
            let st = h.state();
            assert_eq!((st.data, st.cache.as_str(), st.busy), (4242, "", false), "crash {crash}: the same either way");
            // After a crash the replayed log still had them: the boot's reset
            // is an event, so replay and synced copies follow it. A clean
            // stop's snapshot already left them out: nothing to reset.
            let last = h.store(|s| s.events().unwrap()).pop().unwrap().method;
            assert_eq!(last, if crash { "$restore" } else { "begin" });
            assert_eq!(h.store(|s| s.state_at(s.seq()).unwrap()), *st);
            h.begin().unwrap();
        }
    }

    #[test]
    fn a_rewind_writes_and_brings_back_only_what_is_saved() {
        let (_d, p) = tmp();
        let h = Handle::<Work>::open(&p).unwrap();
        h.set(1).unwrap();
        h.set(2).unwrap();
        h.store(|s| s.rewind(&By::Test, 1)).unwrap();
        assert_eq!((h.state().data, h.state().cache.as_str()), (1, ""));
        assert!(!bytes(&p).contains("CACHED"));
        drop(h);
        assert_eq!(Handle::<Work>::open(&p).unwrap().state().data, 1);
    }

    #[test]
    fn replay_is_exact_although_transient_fields_are_never_saved() {
        let (_d, p) = tmp();
        let h = Handle::<Work>::open(&p).unwrap();
        h.set(1).unwrap();
        // Busy at every 256th event: a snapshot there would lose `busy`,
        // and the `finish` after it would no longer replay.
        for _ in 0..300 {
            h.begin().unwrap();
            h.finish().unwrap();
        }
        h.begin().unwrap();
        let want = h.state();
        assert!(!bytes(&p).contains("busy"), "no snapshot could keep it");
        std::mem::forget(h);
        let h = Handle::<Work>::open(&p).unwrap();
        assert_eq!((h.state().done, h.state().busy, h.state().data), (300, false, 1));
        h.store(|s| {
            assert_eq!(s.state_at(602).unwrap(), *want, "time travel still sees the live state");
            assert_eq!(s.state_at(603).unwrap(), *s.state());
            assert!(s.fold_before(i64::MAX).unwrap() > 0);
        });
        h.begin().unwrap();
        h.finish().unwrap();
        let want = h.state();
        drop(h);
        assert_eq!(*Handle::<Work>::open(&p).unwrap().state(), *want);
    }

    /// aio's fix log: undo handles don't survive a restart.
    #[cell(persist, id = "fixlog", restore = repair)]
    pub struct FixLog {
        pub entries: Vec<(u32, bool)>,
    }

    fn repair(mut s: FixLog) -> FixLog {
        s.entries.iter_mut().for_each(|e| e.1 = false);
        s
    }

    #[methods]
    impl FixLog {
        pub fn record(&mut self, id: u32) {
            self.entries.push((id, true));
        }
    }

    #[test]
    fn restore_repairs_the_saved_state_on_every_boot() {
        let (_d, p) = tmp();
        let h = Handle::<FixLog>::open(&p).unwrap();
        h.record(1).unwrap();
        h.record(2).unwrap();
        assert_eq!(h.state().entries, [(1, true), (2, true)], "live state is untouched");
        drop(h);
        let h = Handle::<FixLog>::open(&p).unwrap();
        assert_eq!(h.state().entries, [(1, false), (2, false)]);
        h.record(3).unwrap();
        // A rewind brings saved state back, repaired too.
        h.store(|s| s.rewind(&By::Test, 1)).unwrap();
        assert_eq!(h.state().entries, [(1, false)]);
        std::mem::forget(h);
        let h = Handle::<FixLog>::open(&p).unwrap();
        assert_eq!(h.state().entries, [(1, false)]);
        // Nothing left to repair: no event.
        let n = h.store(|s| s.events().unwrap().len());
        drop(h);
        assert_eq!(Handle::<FixLog>::open(&p).unwrap().store(|s| s.events().unwrap().len()), n);
    }

    #[cell(persist, id = "boom", restore = boom)]
    pub struct Boom {
        pub n: u32,
    }

    fn boom(_: Boom) -> Boom {
        panic!("repair blew up")
    }

    #[methods]
    impl Boom {
        pub fn bump(&mut self) {
            self.n += 1;
        }
    }

    #[test]
    fn a_panicking_restore_is_reported_and_the_app_still_boots() {
        let (_d, p) = tmp();
        let h = Handle::<Boom>::open(&p).unwrap();
        h.bump().unwrap();
        drop(h);
        assert_eq!(Handle::<Boom>::open(&p).unwrap().state().n, 1);
    }

    /// aio's `onPersist` pair: a big live field, a small key on disk.
    #[cell(persist, id = "pic", save = slim, restore = thumb)]
    pub struct Pic {
        pub key: String,
        pub thumb: String,
    }

    fn slim(s: &Pic) -> Pic {
        assert_ne!(s.key, "bad", "a broken save");
        Pic { key: s.key.clone(), thumb: String::new() }
    }

    fn thumb(s: Pic) -> Pic {
        Pic { thumb: format!("thumb of {}", s.key), ..s }
    }

    #[methods]
    impl Pic {
        pub fn show(&mut self, key: String) {
            self.thumb = format!("LIVE-{key}");
            self.key = key;
        }
    }

    #[test]
    fn save_shapes_what_is_written_and_restore_derives_the_rest() {
        for crash in [false, true] {
            let (_d, p) = tmp();
            let h = Handle::<Pic>::open(&p).unwrap();
            h.show("b".into()).unwrap();
            assert_eq!(h.state().thumb, "LIVE-b");
            if crash {
                std::mem::forget(h);
            } else {
                drop(h);
            }
            assert!(!bytes(&p).contains("LIVE"), "crash {crash}: off disk");
            let h = Handle::<Pic>::open(&p).unwrap();
            assert_eq!(h.state().thumb, "thumb of b", "crash {crash}: derived, not the live value");
            drop(h);
            assert_eq!(Handle::<Pic>::open(&p).unwrap().state().thumb, "thumb of b", "repeatable");
        }
    }

    #[test]
    fn a_panicking_save_loses_nothing() {
        let (_d, p) = tmp();
        let h = Handle::<Pic>::open(&p).unwrap();
        h.show("bad".into()).unwrap();
        // Its snapshot fails (reported); the log still holds the call.
        drop(h);
        assert_eq!(Handle::<Pic>::open(&p).unwrap().state().key, "bad");
    }
}

/// aio's `redactActions`: a `#[redact]` method's arguments never reach disk;
/// its event keeps the state it left, so boot and time travel still work.
mod redact {
    use super::{bytes, tmp};
    use aio::prelude::*;

    #[cell(persist, id = "vault")]
    pub struct Vault {
        pub sealed: String,
        pub opens: u32,
        #[transient]
        pub open: bool,
    }

    #[methods]
    impl Vault {
        #[redact]
        pub fn seal(&mut self, cx: &Cx, words: String, pass: String) {
            self.sealed = format!("v3:{}:{}", words.len() + pass.len(), cx.rand() % 1000);
        }

        pub fn open(&mut self) {
            self.open = true;
            self.opens += 1;
        }

        /// Would leave what only its arguments explain in a transient field.
        #[redact]
        pub fn unlock(&mut self, pass: String) -> Result {
            ensure!(!pass.is_empty(), "no passphrase");
            self.open = true;
            Ok(())
        }
    }

    #[test]
    fn redacted_arguments_never_reach_disk_and_the_log_still_replays() {
        assert_eq!(<Vault as Cell>::REDACTED, ["seal", "unlock"]);
        for crash in [false, true] {
            let (_d, p) = tmp();
            let h = Handle::<Vault>::open(&p).unwrap();
            h.seal("abandon ability able".into(), "Correct horse".into()).unwrap();
            let first = h.state().sealed.clone();
            h.open().unwrap();
            // Open: the transient field it can't change is unchanged.
            h.seal("about above absent".into(), "Battery staple".into()).unwrap();
            let want = h.state();
            let evs = h.store(|s| s.events().unwrap());
            assert_eq!(evs.iter().map(|e| e.method.as_str()).collect::<Vec<_>>(), ["seal", "open", "seal"]);
            assert_eq!((evs[0].args[aio::REDACTED].is_object(), evs[1].args.clone()), (true, serde_json::Value::Null));
            h.store(|s| {
                assert_eq!(s.state_at(1).unwrap().sealed, first, "time travel");
                assert_eq!(s.state_at(3).unwrap(), *want);
            });
            if crash {
                std::mem::forget(h);
            } else {
                drop(h);
            }
            let disk = bytes(&p);
            assert!(!disk.contains("abandon") && !disk.contains("horse") && !disk.contains("staple"), "{disk}");
            let h = Handle::<Vault>::open(&p).unwrap();
            assert_eq!((h.state().sealed.as_str(), h.state().opens), (want.sealed.as_str(), 1), "crash {crash}");
        }
    }

    #[test]
    fn a_redacted_method_cannot_change_a_transient_field() {
        let (_d, p) = tmp();
        let h = Handle::<Vault>::open(&p).unwrap();
        let e = h.unlock("Correct horse".into()).unwrap_err();
        assert!(e.why.contains("vault.unlock is #[redact] and can't change what isn't saved"), "{e}");
        assert!(h.store(|s| s.events().unwrap()).is_empty() && !h.state().open, "refused whole");
        h.open().unwrap();
        h.unlock("Correct horse".into()).unwrap();
        assert!(!bytes(&p).contains("horse"));
    }
}

#[test]
fn undo_steps_back_and_redo_walks_the_undos_again() {
    use aio::{Position, Travel};
    let target = |log: &[(u64, Option<u64>)], t| Position::target(log, 0, t);
    // Events 1..3, then undo (4 → 2), undo (5 → 1).
    let mut log: Vec<(u64, Option<u64>)> = vec![(1, None), (2, None), (3, None)];
    assert_eq!(target(&log, Travel::Undo), Ok(2));
    assert!(target(&log, Travel::Redo).unwrap_err().contains("nothing to redo"));
    log.push((4, Some(2)));
    assert_eq!(target(&log, Travel::Undo), Ok(1));
    log.push((5, Some(1)));
    assert_eq!(target(&log, Travel::Redo), Ok(2));
    log.push((6, Some(2)));
    assert_eq!(target(&log, Travel::Redo), Ok(3));
    log.push((7, Some(3)));
    assert!(target(&log, Travel::Redo).unwrap_err().contains("where the undos began"));
    assert_eq!(Position::of(&log, 0), Position { at: 3, head: 3, floor: 0 });
    // A new event after an undo: undo goes to the state it was made on, and redo back to it.
    let mut log: Vec<(u64, Option<u64>)> = vec![(1, None), (2, None), (3, None), (4, Some(1)), (5, None)];
    assert_eq!(target(&log, Travel::Undo), Ok(1), "5 was made on 4's state, which is 1's");
    assert!(target(&log, Travel::Redo).is_err(), "a new event: nothing to redo");
    log.push((6, Some(1)));
    assert_eq!(target(&log, Travel::Redo), Ok(5), "not 2: that was another branch");
    // One sequence numbers every cell's events: 2 and 4 are another cell's.
    let mut log: Vec<(u64, Option<u64>)> = vec![(1, None), (3, None)];
    assert_eq!(target(&log, Travel::Undo), Ok(1), "not 2: that is no event of this cell");
    log.push((5, Some(1)));
    assert_eq!(target(&log, Travel::Undo), Ok(0));
    assert_eq!(target(&log, Travel::Redo), Ok(3));
    assert!(target(&[], Travel::Undo).unwrap_err().contains("nothing to undo"));
    assert!(target(&[(1, None), (2, Some(0))], Travel::Undo).unwrap_err().contains("nothing to undo"));
    // Folded history: nothing before the floor, and goto only within what is kept.
    let log = [(4, None), (5, None)];
    assert_eq!(Position::target(&log, 4, Travel::Undo), Ok(4));
    assert!(Position::target(&[(5, None)], 4, Travel::Undo).unwrap_err().contains("nothing earlier is kept"));
    assert!(Position::target(&log, 4, Travel::To(3)).unwrap_err().contains("no event 3"));
    assert_eq!(Position::target(&log, 4, Travel::To(5)), Ok(5));
}

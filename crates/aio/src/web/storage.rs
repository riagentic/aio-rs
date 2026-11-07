//! What a page keeps in the browser's storage (aio's `browser-storage`): per
//! app and cell one document, the copy's last server view and the calls
//! still waiting for the server, in the shape a native copy saves
//! (`copy.json`). It survives a reload, so calls made offline are sent once
//! the page is back. Pure over [`Kv`], so it is tested without a browser.
//!
//! Twin tabs share one document. A write never takes out another tab's
//! waiting calls: it adds its own to what is stored now, and leaves out
//! only the calls this tab saw the server answer. A call kept by a stale
//! write is merely sent again, and the server applies a call id once.
use crate::Inputs;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

/// Where a page keeps things: `localStorage`, or a map in a test.
pub trait Kv {
    fn get(&self, key: &str) -> Option<String>;
    /// `Err` when the browser refuses it (the quota is full, storage is off).
    fn set(&self, key: &str, value: &str) -> Result<(), String>;
    fn remove(&self, key: &str);
    fn keys(&self) -> Vec<String>;
}

/// A call made on the copy, waiting for the server (the wire's `call`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Queued {
    pub op: String,
    pub method: String,
    pub args: Value,
    #[serde(default)]
    pub inputs: Inputs,
    pub at: i64,
    /// `#[client(predict = false)]`: not shown before the server ran it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub waits: bool,
}

impl Queued {
    /// Its message to the server.
    pub fn msg(&self) -> Value {
        serde_json::json!({ "t": "call", "op": self.op, "method": self.method, "args": self.args })
    }
}

/// One cell's document.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Doc {
    pub cell: String,
    pub v: u32,
    /// Who the waiting calls are by: another user's are never sent as this one.
    #[serde(default)]
    pub user: String,
    pub seq: u64,
    pub view: Value,
    pub pending: Vec<Queued>,
    /// A page cell's schedules (a synced copy has none: its server runs them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sched: Vec<Due>,
}

/// A schedule a page's cell set (`cx.after`, `every`, `at`, `cron`, and its
/// declared ones as `@<method>`), kept in its document: the native store's
/// `sched` row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Due {
    pub key: String,
    /// When it runs next (ms since the epoch).
    pub due: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    pub method: String,
    pub args: Value,
}

/// `have` once `d` fired at `now`, as the native store re-arms (pure): a
/// repeating one moves to its next slot after `now` (missed slots fire
/// once), a one-shot goes. Only the one that fired: one its method set
/// again meanwhile keeps its own time.
pub fn rearm(have: &mut Vec<Due>, d: &Due, now: i64) {
    let Some(i) = have.iter().position(|x| x.key == d.key && x.due == d.due) else { return };
    match crate::cx::next_run(d.due, d.every, d.cron.as_deref(), now) {
        Some(next) => have[i].due = next,
        None => _ = have.remove(i),
    }
}

/// `have` with a cell's declared schedules (`#[every]`, `#[cron]`) armed as
/// `@<method>`, as the native store arms them (pure): one kept with the same
/// timing keeps its next run, a changed one starts over from `now`, and an
/// `@` one no longer declared goes.
pub fn declare(have: &mut Vec<Due>, declared: &[crate::Declared], now: i64) -> Result<(), String> {
    have.retain(|d| d.key.strip_prefix('@').is_none_or(|m| declared.iter().any(|s| s.method == m)));
    for s in declared {
        let key = format!("@{}", s.method);
        if have.iter().any(|d| d.key == key && d.every == s.every && d.cron.as_deref() == s.cron) {
            continue;
        }
        let due = match (s.every, s.cron) {
            (Some(ms), _) => now + ms,
            (None, Some(c)) => {
                let c = crate::cron::Cron::parse(c).map_err(|e| format!("{}: #[cron]: {e}", s.method))?;
                c.next(now).ok_or_else(|| format!("{}: #[cron] never runs", s.method))?
            }
            (None, None) => continue,
        };
        have.retain(|d| d.key != key);
        let cron = s.cron.map(String::from);
        have.push(Due { key, due, every: s.every, cron, method: s.method.into(), args: Value::Null });
    }
    Ok(())
}

/// The namespace of app `app`'s documents. `localStorage` is per origin, not
/// per app: two apps served from one host and port must not share a queue,
/// so the app's id is part of every key, made unable to reshape one.
pub fn prefix(app: &str) -> String {
    let id: String =
        app.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    format!("aio.{id}")
}

/// Cell `cell`'s document key in app `app`.
pub fn key(app: &str, cell: &str) -> String {
    format!("{}:{cell}", prefix(app))
}

/// Where a document that can't be read is kept, for whoever wants to look.
fn corrupt(key: &str) -> String {
    format!("{key}.corrupt")
}

/// The document at `key`; one that isn't a document (broken JSON, or JSON
/// of another shape) is kept at `<key>.corrupt` and said, then counts as
/// none: what it held is lost, but never silently.
pub fn load(kv: &impl Kv, key: &str, say: impl Fn(&str)) -> Option<Doc> {
    let raw = kv.get(key)?;
    match serde_json::from_str::<Doc>(&raw) {
        Ok(d) => Some(d),
        Err(e) => {
            _ = kv.set(&corrupt(key), &raw);
            say(&format!(
                "the offline queue at \"{key}\" can't be read ({e}): its unsent calls are lost; the raw document \
                 is kept at \"{}\"",
                corrupt(key)
            ));
            kv.remove(key);
            None
        }
    }
}

/// `d` (at `key`) when it is cell `cell`'s at version `v`; else `None`,
/// and calls still waiting in it are kept at `<key>.orphan-v<n>` (then
/// `-2`, `-3`…) and said
/// (aio's `sync-orphan-queue`), never dropped silently.
pub fn current(kv: &impl Kv, key: &str, d: Doc, cell: &str, v: u32, say: impl Fn(&str)) -> Option<Doc> {
    if d.cell == cell && d.v == v {
        return Some(d);
    }
    if !d.pending.is_empty() {
        // Never over an earlier one (the same version orphaned twice).
        let base = format!("{key}.orphan-v{}", d.v);
        let aside = std::iter::once(base.clone())
            .chain((2..).map(|i| format!("{base}-{i}")))
            .find(|k| kv.get(k).is_none())
            .unwrap_or(base);
        let kept = match kv.set(&aside, &serde_json::to_string(&d).unwrap_or_default()) {
            Ok(()) => format!("kept at \"{aside}\""),
            Err(e) => format!("not kept: {e}"),
        };
        say(&format!(
            "{cell}: {} calls made offline on {} version {} never reached the server and this build can't send them \
             (version {v}): {kept}",
            d.pending.len(),
            d.cell,
            d.v
        ));
    }
    None
}

/// `mine` and the calls of `theirs` that are not mine and that this tab
/// never saw answered (`done`), oldest first (pure).
pub fn merge(mine: &[Queued], theirs: &[Queued], done: &HashSet<String>) -> Vec<Queued> {
    let have: HashSet<&str> = mine.iter().map(|q| q.op.as_str()).collect();
    let mut all: Vec<Queued> = mine.to_vec();
    all.extend(theirs.iter().filter(|q| !have.contains(q.op.as_str()) && !done.contains(&q.op)).cloned());
    all.sort_by(|a, b| (a.at, &a.op).cmp(&(b.at, &b.op)));
    all
}

/// The calls of `theirs` this tab doesn't have and never saw answered:
/// what a twin tab queued, to show and send here too (pure).
pub fn adopt(mine: &[Queued], theirs: &Doc, user: &str, done: &HashSet<String>) -> Vec<Queued> {
    if theirs.user != user {
        return Vec::new();
    }
    let have: HashSet<&str> = mine.iter().map(|q| q.op.as_str()).collect();
    theirs.pending.iter().filter(|q| !have.contains(q.op.as_str()) && !done.contains(&q.op)).cloned().collect()
}

/// Write `doc` at `key`, keeping the calls another tab stored meanwhile
/// (theirs, as by [`merge`]) and the newer server view of the two. Another
/// user's document is replaced only once its calls are none. `Err` when the
/// browser refused it: the copy goes on in memory.
pub fn save(kv: &impl Kv, key: &str, mut doc: Doc, done: &HashSet<String>, say: impl Fn(&str)) -> Result<(), String> {
    if let Some(theirs) = load(kv, key, &say) {
        if theirs.user != doc.user && !theirs.pending.is_empty() {
            return Err(format!(
                "\"{key}\" holds {} unsent calls by {}: they are kept, and not sent as {}",
                theirs.pending.len(),
                theirs.user,
                doc.user
            ));
        }
        if theirs.user == doc.user {
            doc.pending = merge(&doc.pending, &theirs.pending, done);
            if theirs.seq > doc.seq && theirs.v == doc.v {
                (doc.seq, doc.view) = (theirs.seq, theirs.view);
            }
        }
    }
    let text = serde_json::to_string(&doc).map_err(|e| e.to_string())?;
    kv.set(key, &text)
}

/// The cells of app `app` whose stored calls no copy here will ever send:
/// a document for a cell not in `syncing` (its sync dropped, the cell
/// removed) with calls waiting, and how many (aio's `orphanSyncQueues`).
pub fn orphans(kv: &impl Kv, app: &str, syncing: &[&str]) -> Vec<(String, usize)> {
    let p = format!("{}:", prefix(app));
    let mut out: Vec<(String, usize)> = kv
        .keys()
        .into_iter()
        .filter_map(|k| {
            let cell = k.strip_prefix(&p)?;
            if cell.ends_with(".corrupt") || syncing.contains(&cell) {
                return None;
            }
            let n = serde_json::from_str::<Doc>(&kv.get(&k)?).ok()?.pending.len();
            (n > 0).then(|| (cell.to_string(), n))
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Map {
        m: RefCell<BTreeMap<String, String>>,
        full: std::cell::Cell<bool>,
    }

    impl Kv for Map {
        fn get(&self, key: &str) -> Option<String> {
            self.m.borrow().get(key).cloned()
        }
        fn set(&self, key: &str, value: &str) -> Result<(), String> {
            if self.full.get() {
                return Err("QuotaExceededError".into());
            }
            self.m.borrow_mut().insert(key.into(), value.into());
            Ok(())
        }
        fn remove(&self, key: &str) {
            self.m.borrow_mut().remove(key);
        }
        fn keys(&self) -> Vec<String> {
            self.m.borrow().keys().cloned().collect()
        }
    }

    fn q(op: &str, at: i64) -> Queued {
        Queued { op: op.into(), method: "add".into(), args: Value::Null, inputs: Inputs::default(), at, waits: false }
    }

    #[test]
    fn a_queue_of_another_version_is_kept_aside_and_said() {
        let (kv, said) = (Map::default(), RefCell::new(vec![]));
        let say = |m: &str| said.borrow_mut().push(m.to_string());
        assert!(current(&kv, "k", doc("u", 1, vec![q("a", 1)]), "todos", 1, say).is_some());
        assert!(current(&kv, "k", doc("u", 1, vec![q("a", 1)]), "todos", 2, say).is_none());
        assert!(kv.get("k.orphan-v1").unwrap().contains("\"op\":\"a\""));
        assert!(said.borrow()[0].contains("1 calls made offline on todos version 1"), "{said:?}");
        assert!(current(&kv, "k", doc("u", 1, vec![]), "todos", 2, say).is_none());
        assert_eq!(said.borrow().len(), 1, "an empty queue loses nothing");
        assert!(current(&kv, "k", doc("u", 1, vec![q("b", 2)]), "todos", 2, say).is_none());
        assert!(kv.get("k.orphan-v1").unwrap().contains("\"op\":\"a\""), "the first set stays");
        assert!(kv.get("k.orphan-v1-2").unwrap().contains("\"op\":\"b\""));
    }

    fn doc(user: &str, seq: u64, pending: Vec<Queued>) -> Doc {
        let view = serde_json::json!({ "seq": seq });
        Doc { cell: "todos".into(), v: 1, user: user.into(), seq, view, pending, sched: Vec::new() }
    }

    fn ops(kv: &Map, key: &str) -> Vec<String> {
        load(kv, key, |_| {}).map(|d| d.pending.into_iter().map(|q| q.op).collect()).unwrap_or_default()
    }

    const K: &str = "aio.app:todos";
    fn none() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn each_app_has_its_own_keys_and_none_can_reshape_another() {
        // aio storage-prefix: two apps on one origin never share a queue.
        assert_eq!(key("todo", "todos"), "aio.todo:todos");
        assert_ne!(key("a", "x"), key("b", "x"));
        assert_eq!(prefix("evil:app/x"), "aio.evil_app_x", "no separator gets in");
    }

    #[test]
    fn twin_tabs_queueing_at_once_lose_no_call() {
        // aio: two tabs offline, each writing what it read: one's call was lost (1 of 120).
        let kv = Map::default();
        // Both read nothing, then each writes its own call.
        save(&kv, K, doc("ann", 1, vec![q("a1", 10)]), &none(), |_| {}).unwrap();
        save(&kv, K, doc("ann", 1, vec![q("b1", 11)]), &none(), |_| {}).unwrap();
        assert_eq!(ops(&kv, K), ["a1", "b1"], "the second write kept the first's call");
        // A tab hears the other's write: it adopts what it lacks.
        let theirs = load(&kv, K, |_| {}).unwrap();
        assert_eq!(adopt(&[q("a1", 10)], &theirs, "ann", &none()), [q("b1", 11)]);
    }

    #[test]
    fn a_call_seen_answered_is_never_put_back_by_its_tab() {
        let kv = Map::default();
        save(&kv, K, doc("ann", 1, vec![q("a1", 10), q("a2", 12)]), &none(), |_| {}).unwrap();
        // The server answered a1: this tab writes without it, and it stays out.
        let done: HashSet<String> = ["a1".to_string()].into();
        save(&kv, K, doc("ann", 2, vec![q("a2", 12)]), &done, |_| {}).unwrap();
        assert_eq!(ops(&kv, K), ["a2"]);
        let theirs = load(&kv, K, |_| {}).unwrap();
        assert!(adopt(&[], &theirs, "ann", &done).iter().all(|c| c.op != "a1"));
    }

    #[test]
    fn the_newer_server_view_of_two_tabs_is_kept() {
        let kv = Map::default();
        save(&kv, K, doc("ann", 9, vec![]), &none(), |_| {}).unwrap();
        save(&kv, K, doc("ann", 4, vec![q("a1", 1)]), &none(), |_| {}).unwrap();
        let d = load(&kv, K, |_| {}).unwrap();
        assert_eq!((d.seq, d.view["seq"].as_u64()), (9, Some(9)), "a stale tab doesn't roll the copy back");
        assert_eq!(ops(&kv, K), ["a1"]);
    }

    #[test]
    fn another_users_unsent_calls_are_kept_and_never_sent_as_this_one() {
        let kv = Map::default();
        save(&kv, K, doc("ann", 1, vec![q("a1", 1)]), &none(), |_| {}).unwrap();
        let theirs = load(&kv, K, |_| {}).unwrap();
        assert!(adopt(&[], &theirs, "bob", &none()).is_empty(), "bob never adopts ann's call");
        assert!(save(&kv, K, doc("bob", 5, vec![q("b1", 2)]), &none(), |_| {}).is_err());
        assert_eq!(ops(&kv, K), ["a1"], "ann's call is still there for ann");
        // Once ann's are gone, bob's document replaces hers.
        save(&kv, K, doc("ann", 2, vec![]), &["a1".to_string()].into(), |_| {}).unwrap();
        save(&kv, K, doc("bob", 5, vec![q("b1", 2)]), &none(), |_| {}).unwrap();
        assert_eq!(load(&kv, K, |_| {}).unwrap().user, "bob");
    }

    #[test]
    fn a_corrupt_document_is_kept_aside_and_said_never_silently_overwritten() {
        // aio: `null`, `[]`, `42`, `{ops: "x"}` and broken JSON are all corrupt.
        for raw in ["{broken", "null", "[]", "42", r#"{"pending":"x"}"#] {
            let kv = Map::default();
            kv.set(K, raw).unwrap();
            let said = RefCell::new(Vec::new());
            assert_eq!(load(&kv, K, |m| said.borrow_mut().push(m.to_string())), None, "{raw}");
            assert_eq!(kv.get("aio.app:todos.corrupt").as_deref(), Some(raw), "{raw}");
            assert!(said.borrow()[0].contains("unsent calls are lost"), "{raw}: {:?}", said.borrow());
            // The next write starts afresh; the copy stays.
            save(&kv, K, doc("ann", 1, vec![q("a1", 1)]), &none(), |_| {}).unwrap();
            assert_eq!(ops(&kv, K), ["a1"]);
            assert_eq!(kv.get("aio.app:todos.corrupt").as_deref(), Some(raw));
        }
    }

    #[test]
    fn a_refused_write_is_an_error_and_leaves_the_stored_document() {
        let kv = Map::default();
        save(&kv, K, doc("ann", 1, vec![q("a1", 1)]), &none(), |_| {}).unwrap();
        kv.full.set(true);
        let e = save(&kv, K, doc("ann", 2, vec![q("a1", 1), q("a2", 2)]), &none(), |_| {}).unwrap_err();
        assert!(e.contains("Quota"), "{e}");
        assert_eq!(ops(&kv, K), ["a1"]);
    }

    #[test]
    fn orphan_queues_are_the_unsent_calls_of_cells_no_copy_syncs() {
        // aio orphanSyncQueues.
        let kv = Map::default();
        save(&kv, &key("app", "todos"), doc("ann", 1, vec![q("a1", 1)]), &none(), |_| {}).unwrap();
        save(&kv, &key("app", "old"), doc("ann", 1, vec![q("o1", 1), q("o2", 2)]), &none(), |_| {}).unwrap();
        save(&kv, &key("app", "empty"), doc("ann", 1, vec![]), &none(), |_| {}).unwrap();
        save(&kv, &key("other", "old"), doc("ann", 1, vec![q("x", 1)]), &none(), |_| {}).unwrap();
        kv.set("aio.app:gone.corrupt", "{").unwrap();
        assert_eq!(orphans(&kv, "app", &["todos"]), [("old".to_string(), 2)]);
        assert!(orphans(&kv, "app", &["todos", "old"]).is_empty());
    }

    fn due(key: &str, due: i64, every: Option<i64>, cron: Option<&str>) -> Due {
        Due { key: key.into(), due, every, cron: cron.map(String::from), method: "tick".into(), args: Value::Null }
    }

    #[test]
    fn a_fired_schedule_rearms_as_natively_missed_slots_once_a_one_shot_goes() {
        let mut have = vec![due("poll", 1000, Some(100), None), due("once", 1000, None, None)];
        let (poll, once) = (have[0].clone(), have[1].clone());
        rearm(&mut have, &poll, 1050);
        assert_eq!(have[0].due, 1100, "its next slot");
        rearm(&mut have, &due("poll", 1100, Some(100), None), 1450);
        assert_eq!(have[0].due, 1550, "missed slots fire once, then on from now");
        rearm(&mut have, &once, 1000);
        assert_eq!(have.len(), 1, "a one-shot goes");
        // Set again meanwhile (another due): what fired doesn't touch it.
        rearm(&mut have, &poll, 2000);
        assert_eq!(have[0].due, 1550);
        // A cron one: its next slot after now (a minute's).
        let mut c = vec![due("c", 60_000, None, Some("* * * * *"))];
        let fired = c[0].clone();
        rearm(&mut c, &fired, 150_000);
        assert_eq!(c[0].due, 180_000);
    }

    #[test]
    fn declared_schedules_arm_as_at_method_keep_their_time_and_go_once_undeclared() {
        let d = |method, every, cron| crate::Declared { method, every, cron, skip_if_running: false };
        let mut have = vec![due("mine", 5, None, None), due("@gone", 5, Some(10), None)];
        declare(&mut have, &[d("tick", Some(1000), None), d("daily", None, Some("0 9 * * *"))], 0).unwrap();
        let keys: Vec<&str> = have.iter().map(|d| d.key.as_str()).collect();
        assert_eq!(keys, ["mine", "@tick", "@daily"], "an app-set one stays; an undeclared @ one goes");
        assert_eq!((have[1].due, have[2].due), (1000, 9 * 3_600_000));
        // Booted again later with the same timing: the next run is kept.
        declare(&mut have, &[d("tick", Some(1000), None), d("daily", None, Some("0 9 * * *"))], 500).unwrap();
        assert_eq!(have[1].due, 1000);
        // A changed timing starts over from now.
        declare(&mut have, &[d("tick", Some(2000), None)], 500).unwrap();
        assert_eq!((have.len(), have[1].key.as_str(), have[1].due), (2, "@tick", 2500));
        assert!(declare(&mut have, &[d("bad", None, Some("nope"))], 0).unwrap_err().contains("bad: #[cron]"));
    }

    #[test]
    fn a_document_keeps_its_schedules_and_one_saved_before_them_reads() {
        let kv = Map::default();
        let mut d = doc("", 0, vec![]);
        d.sched = vec![due("poll", 1000, Some(100), None)];
        save(&kv, K, d.clone(), &none(), |_| {}).unwrap();
        assert_eq!(load(&kv, K, |_| {}).unwrap().sched, d.sched);
        kv.set(K, r#"{"cell":"todos","v":1,"seq":0,"view":null,"pending":[]}"#).unwrap();
        assert!(load(&kv, K, |_| panic!("read")).unwrap().sched.is_empty());
        assert!(!serde_json::to_string(&doc("", 0, vec![])).unwrap().contains("sched"), "a copy's stays as it was");
    }
}

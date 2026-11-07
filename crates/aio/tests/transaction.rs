//! Transactions (`#[cell(transaction)]`, aio's `transaction: true`,
//! tests/transactional-methods.test.ts): an async method reads the state as
//! it was when it was called, and commits what it wrote at its end, at once,
//! or nothing; a write another call changed meanwhile refuses it.
use aio::prelude::*;
use aio::testing::TestCells;

/// Stands in for a quote service.
async fn fetch(n: i64) -> i64 {
    n
}

#[cell(transaction)]
pub struct Wallet {
    pub balance: i64,
    pub quote: i64,
    pub status: String,
    pub log: Vec<String>,
}

#[methods]
impl Wallet {
    pub fn adjust(&mut self, by: i64) {
        self.balance += by;
    }

    pub fn requote(&mut self, q: i64) {
        self.quote = q;
    }

    pub fn note(&mut self, s: String) {
        self.log.push(s);
    }

    /// aio's field report: a refresh that computes from a pinned read.
    pub async fn refresh(&mut self, cx: &Cx, to: i64) -> Result<i64> {
        self.status = "refreshing".into();
        let seen: i64 = self.balance;
        let got: i64 = fetch(to).await;
        ensure!(self.balance == seen, "pinned reads never move");
        if let Some(now) = cx.live_state::<Self>() {
            self.log.push(format!("live {}", now.balance));
        }
        self.balance = got;
        self.status = "done".into();
        Ok(got)
    }

    /// Writes only `quote`: disjoint from `adjust`'s `balance`.
    pub async fn price(&mut self, to: i64) -> Result {
        let q: i64 = fetch(to).await;
        self.quote = q;
        Ok(())
    }

    /// Writes, asks for an effect and a schedule, then refuses: none of it stays.
    pub async fn fail(&mut self, cx: &Cx) -> Result {
        self.status = "failing".into();
        cx.after("later", 1_000, "note", ("scheduled",))?;
        cx.run(Effect::new("side", |h: Handle<Wallet>| async move { _ = h.note("effect ran".into()) }));
        let _: i64 = fetch(1).await;
        reject!("refused after the await");
    }

    /// Publishes its spinner before the await (aio's `s.$commit()`).
    pub async fn spin(&mut self, cx: &Cx) -> Result {
        self.status = "spinning".into();
        cx.commit();
        let _: i64 = fetch(1).await;
        self.status = format!("spun at {}", self.balance);
        Ok(())
    }
}

#[test]
fn reads_are_pinned_and_the_writes_commit_at_once_at_the_end() {
    let mut t = TestCells::new();
    let h = t.cell::<Wallet>();
    h.adjust(5).unwrap();
    let mut a = h.refresh(40).unwrap();
    assert_eq!(h.state().status, "", "nothing it wrote is seen before its end");
    h.note("meanwhile".into()).unwrap();
    t.settle();
    assert_eq!(a.now().unwrap().unwrap(), 40);
    let s = h.state();
    assert_eq!((s.balance, s.status.as_str()), (40, "done"));
    assert_eq!(s.log, ["meanwhile", "live 5"], "the other call's write stays; live reads see the state now");
    let methods: Vec<String> = h.store(|s| s.events().unwrap()).into_iter().map(|e| e.method).collect();
    assert_eq!(methods, ["adjust", "note", "refresh"], "one event for the whole call");
    // Replay brings the same state back (the event keeps the state it left).
    let (seq, again) = h.store(|s| (s.seq(), s.state_at(s.seq()).unwrap()));
    assert!(seq > 0);
    assert_eq!(again, *h.state());
}

#[test]
fn a_write_another_call_changed_meanwhile_refuses_with_tx_conflict() {
    let mut t = TestCells::new();
    let h = t.cell::<Wallet>();
    let mut a = h.refresh(40).unwrap();
    h.adjust(7).unwrap(); // the user's transfer lands during the fetch
    t.settle();
    let e = a.now().unwrap().unwrap_err();
    assert_eq!(e.code(), Some(Code::TxConflict), "{e}");
    assert!(e.why.contains("self.balance was changed by another call"), "{e}");
    let s = h.state();
    assert_eq!((s.balance, s.status.as_str()), (7, ""), "the transfer stands; nothing of the refresh committed");
    let f = t.failures();
    assert!(f.iter().any(|f| f.contains("TX_CONFLICT") || f.contains("changed by another call")), "{f:?}");
}

#[test]
fn disjoint_writes_both_land() {
    let mut t = TestCells::new();
    let h = t.cell::<Wallet>();
    let mut a = h.price(9).unwrap();
    h.adjust(3).unwrap();
    t.settle();
    a.now().unwrap().unwrap();
    assert_eq!((h.state().balance, h.state().quote), (3, 9));
}

#[test]
fn a_blind_write_never_conflicts_the_last_write_wins() {
    let mut t = TestCells::new();
    let h = t.cell::<Wallet>();
    let mut a = h.price(9).unwrap(); // assigns quote, never reads it
    h.requote(4).unwrap();
    t.settle();
    a.now().unwrap().unwrap();
    assert_eq!(h.state().quote, 9, "aio's read tracking: a write from no read overwrites");
}

#[test]
fn a_refusal_drops_what_it_wrote_and_asked_for() {
    let mut t = TestCells::new();
    let h = t.cell::<Wallet>();
    let mut a = h.fail().unwrap();
    t.settle();
    assert!(a.now().unwrap().unwrap_err().why.contains("refused after the await"));
    t.advance(2_000);
    let s = h.state();
    assert_eq!(s.status, "");
    assert!(s.log.is_empty(), "neither its effect nor its schedule ran: {:?}", s.log);
    assert!(h.store(|s| s.schedules().unwrap()).is_empty());
    _ = t.failures();
}

#[test]
fn commit_publishes_mid_method_and_reads_go_on_from_there() {
    let mut t = TestCells::new();
    let h = t.cell::<Wallet>();
    let mut a = h.spin().unwrap();
    assert_eq!(h.state().status, "spinning", "published before the await");
    h.adjust(2).unwrap();
    t.settle();
    a.now().unwrap().unwrap();
    // `balance` was not written by it: no conflict, and it read the state at its publish.
    assert_eq!((h.state().balance, h.state().status.as_str()), (2, "spun at 0"));
}

#[cell(transaction(serialize))]
pub struct Ledger {
    pub n: i64,
    pub other: i64,
}

#[methods]
impl Ledger {
    /// A read-modify-write across an await.
    pub async fn bump(&mut self) -> Result<i64> {
        let n: i64 = self.n;
        let _: i64 = fetch(1).await;
        self.n = n + 1;
        Ok(self.n)
    }

    /// Reads `other`, writes `n` without reading it.
    pub async fn tally(&mut self) -> Result {
        let o: i64 = self.other;
        let _: i64 = fetch(1).await;
        self.n = o + 1;
        Ok(())
    }

    pub fn touch(&mut self) {
        self.other += 1;
    }

    pub fn stop(&mut self, cx: &Cx) {
        cx.abort("bump");
    }
}

#[test]
fn serialized_calls_run_one_at_a_time_and_lose_no_update() {
    let mut t = TestCells::new();
    let h = t.cell::<Ledger>();
    let (mut a, mut b) = (h.bump().unwrap(), h.bump().unwrap());
    t.settle();
    assert_eq!((a.now().unwrap().unwrap(), b.now().unwrap().unwrap()), (1, 2));
    assert_eq!(h.state().n, 2, "the second read after the first committed");
}

#[test]
fn serialized_a_change_to_anything_read_meanwhile_is_a_conflict() {
    let mut t = TestCells::new();
    let h = t.cell::<Ledger>();
    let mut a = h.tally().unwrap();
    h.touch().unwrap(); // a sync method isn't held off, and every read counts, not only the written
    t.settle();
    assert_eq!(a.now().unwrap().unwrap_err().code(), Some(Code::TxConflict));
    assert_eq!((h.state().n, h.state().other), (0, 1));
    // A field it never read moving meanwhile is none of its business (aio's).
    let mut b = h.bump().unwrap();
    h.touch().unwrap();
    t.settle();
    assert_eq!(b.now().unwrap().unwrap(), 1);
    assert_eq!((h.state().n, h.state().other), (1, 2));
    _ = t.failures();
}

#[test]
fn an_aborted_serialized_call_gives_the_next_its_turn() {
    let mut t = TestCells::new();
    let h = t.cell::<Ledger>();
    let mut a = h.bump().unwrap();
    let mut b = h.bump().unwrap();
    h.stop().unwrap(); // aborts the first's await: it ends there, writing nothing
    t.settle();
    assert!(a.now().unwrap().is_err(), "the aborted one is told");
    assert_eq!(b.now().unwrap().unwrap(), 1);
    assert_eq!(h.state().n, 1);
    _ = t.failures();
}

#[cell(transaction(conflict = "warn"))]
pub struct Loose {
    pub n: i64,
}

#[methods]
impl Loose {
    pub fn set(&mut self, n: i64) {
        self.n = n;
    }

    pub async fn bump(&mut self) -> Result {
        let n: i64 = self.n;
        let _: i64 = fetch(1).await;
        self.n = n + 10;
        Ok(())
    }
}

#[test]
fn conflict_warn_commits_anyway() {
    let mut t = TestCells::new();
    let h = t.cell::<Loose>();
    let mut a = h.bump().unwrap();
    h.set(5).unwrap();
    t.settle();
    a.now().unwrap().unwrap();
    assert_eq!(h.state().n, 10, "said, and committed over the other call's write");
}

/// Without the flag, an async method keeps aio's live reads: each part commits.
#[cell]
pub struct Live {
    pub n: i64,
    pub status: String,
}

#[methods]
impl Live {
    pub fn set(&mut self, n: i64) {
        self.n = n;
    }

    pub async fn bump(&mut self) -> Result {
        self.status = "busy".into();
        let _: i64 = fetch(1).await;
        self.n += 10;
        Ok(())
    }
}

#[test]
fn a_cell_without_the_flag_commits_each_part() {
    let mut t = TestCells::new();
    let h = t.cell::<Live>();
    let _a = h.bump().unwrap();
    assert_eq!(h.state().status, "busy");
    h.set(5).unwrap();
    t.settle();
    assert_eq!(h.state().n, 15);
}

/// Stands in for slow I/O.
async fn slow(ms: u64) -> i64 {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    1
}

#[cell(persist, id = "purse", transaction(serialize))]
pub struct Purse {
    pub n: i64,
}

#[methods]
impl Purse {
    pub async fn bump(&mut self, ms: u64) -> Result<i64> {
        let n: i64 = self.n;
        let _: i64 = slow(ms).await;
        self.n = n + 1;
        Ok(self.n)
    }

    pub fn stop(&mut self, cx: &Cx) {
        cx.abort("bump");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn in_an_aio_file_a_running_transaction_aborted_frees_the_turn_and_aio_reads_the_commit() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.db");
    let h = Handle::<Purse>::open_aio(&p, "app").unwrap();
    let a = h.bump(10_000).unwrap();
    let b = h.bump(10).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(h.state().n, 0, "the second waits for the first's turn");
    h.stop().unwrap(); // the first's task is aborted: its turn ends with it
    assert!(a.await.is_err());
    assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(5), b).await.unwrap().unwrap(), 1);
    assert_eq!(h.state().n, 1);
    drop(h);
    // aio's own slice holds the commit; aio-rs boots it back.
    let c = rusqlite::Connection::open(&p).unwrap();
    let vals: Vec<String> =
        c.prepare("SELECT v FROM aio_kv").unwrap().query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
    assert!(vals.iter().any(|v| v.contains(r#""n":1"#)), "{vals:?}");
    drop(c);
    assert_eq!(Handle::<Purse>::open_aio(&p, "app").unwrap().state().n, 1);
}

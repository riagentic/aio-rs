//! Async methods: written in one piece like aio, run as method + effect +
//! step methods, so the log holds every awaited result and replay is exact.
use aio::prelude::*;
use std::time::Duration;

#[cell(persist)]
pub struct Shop {
    pub status: String,
    pub total: u64,
    pub order: Option<String>,
    pub notes: Vec<String>,
}

/// Fake I/O that answers differently each time: a replay that asked again
/// would compute another state.
async fn quote(item: String) -> u64 {
    tokio::time::sleep(Duration::from_millis(20)).await;
    item.len() as u64 * 100 + aio::uuid_v4().as_u64_pair().0 % 50
}

async fn place(item: String, price: u64) -> String {
    tokio::time::sleep(Duration::from_millis(5)).await;
    format!("{item}@{price}#{}", aio::uuid_v4().simple())
}

#[methods]
impl Shop {
    /// aio: `async checkout(s, item, limit) { s.status = "pricing"; const price = await quote(item); … }`
    pub async fn checkout(&mut self, cx: &Cx, item: String, limit: u64) -> Result {
        ensure!(!item.is_empty(), "nothing to buy");
        self.status = "pricing".into();
        let started: i64 = cx.now();
        let price: u64 = quote(item.clone()).await;
        self.status = "priced".into();
        ensure!(price <= limit, "{item} costs {price}, over {limit}");
        self.total += price;
        let order: String = place(item, price).await;
        let id = cx.id();
        self.order = Some(format!("{id}:{order}"));
        self.status = format!("done in {} ms", cx.now() - started);
        Ok(())
    }

    /// No `cx`, and a refusal after the last await.
    pub async fn note(&mut self, text: String) -> Result {
        let mut shout: String = text.to_uppercase();
        let echo: String = async move { text }.await;
        shout.push('!');
        ensure!(echo != "no", "refused after the await");
        self.notes.push(format!("{echo}/{shout}"));
        Ok(())
    }

    pub fn plain(&mut self) {
        self.notes.push("plain".into());
    }
}

fn methods(h: &Handle<Shop>) -> Vec<String> {
    h.store(|s| s.events().unwrap()).into_iter().map(|e| e.method).collect()
}

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..300 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out: {what}");
}

#[test]
fn steps_are_hidden_and_the_method_stays_typed() {
    assert_eq!(<Shop as Cell>::METHODS, ["checkout", "note", "plain"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_async_method_commits_each_part_and_records_each_result() {
    let h = Handle::<Shop>::memory();
    h.checkout("book".into(), 1000).unwrap();
    assert_eq!(h.state().status, "pricing", "the part before the await is committed and visible");
    until("done", || h.state().status.starts_with("done")).await;
    let s = h.state();
    assert!((400..450).contains(&s.total), "{}", s.total);
    assert!(s.order.as_ref().unwrap().contains(&format!("book@{}#", s.total)));
    let ev = h.store(|s| s.events().unwrap());
    assert_eq!(methods(&h), ["checkout", "checkout.1", "checkout.2"]);
    assert_eq!(ev[1].by, By::Effect("checkout".into()));
    assert_eq!(ev[1].args, serde_json::json!(["book", 1000, ev[0].at, s.total]), "args, kept lets, the result");
    assert_eq!(ev[2].inputs.ids.len(), 1, "a step's own ids are recorded");

    h.note("hi".into()).unwrap();
    until("noted", || h.state().notes.len() == 1).await;
    assert_eq!(h.state().notes, ["hi/HI!"], "a `mut` let lives across the await");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_first_part_commits_nothing_and_starts_no_io() {
    let h = Handle::<Shop>::memory();
    assert_eq!(h.checkout(String::new(), 1000), Err(Rejected::new("nothing to buy")));
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!h.busy("checkout"));
    assert!(methods(&h).is_empty());
    assert_eq!(*h.state(), Shop::default());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_step_keeps_the_parts_before_and_ends_the_method() {
    let h = Handle::<Shop>::memory();
    h.checkout("television".into(), 10).unwrap();
    until("step ran", || !h.busy("checkout")).await;
    assert_eq!(methods(&h), ["checkout"], "the refused step left no event, the order was never placed");
    let s = h.state();
    assert_eq!(s.status, "pricing", "the step's own writes are dropped (aio kept them)");
    assert_eq!((s.total, s.order.clone()), (0, None));

    h.note("no".into()).unwrap();
    until("note ran", || !h.busy("note")).await;
    assert_eq!(methods(&h), ["checkout", "note"]);
    assert!(h.state().notes.is_empty());
}

#[test]
fn replay_is_exact_and_does_no_io() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("s.db");
    let live = {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let h = Handle::<Shop>::open(&p).unwrap();
        let _g = rt.enter();
        h.checkout("book".into(), 1000).unwrap();
        h.checkout("pen".into(), 1000).unwrap();
        rt.block_on(until("both", || h.state().order.as_ref().is_some_and(|o| o.contains("pen@"))));
        h.state()
    };
    // No runtime: nothing could run, yet the random answers come back.
    let h = Handle::<Shop>::open(&p).unwrap();
    assert_eq!(h.state(), live);
    assert_eq!(methods(&h), ["checkout", "checkout", "checkout.1", "checkout.1", "checkout.2", "checkout.2"]);
}

/// `#[ttl(ms)]`: aio's `ttl: { load: ms }`, on the virtual clock of `TestCells`.
#[cfg(feature = "testing")]
mod ttl {
    use aio::prelude::*;
    use aio::testing::TestCells;

    #[cell]
    pub struct Users {
        pub names: Vec<String>,
    }

    async fn lookup(id: u32) -> String {
        format!("user-{id}")
    }

    #[methods]
    impl Users {
        #[ttl(60_000)]
        pub async fn load(&mut self, id: u32) -> Result {
            let name: String = lookup(id).await;
            ensure!(id != 0, "no user 0");
            self.names.push(name);
            Ok(())
        }

        #[ttl(1000)]
        pub async fn pair(&mut self, a: u32, b: u32) -> Result {
            let x: String = lookup(a).await;
            let y: String = lookup(b).await;
            self.names.push(format!("{x}+{y}"));
            Ok(())
        }
    }

    fn runs(h: &Handle<Users>) -> usize {
        h.store(|s| s.events().unwrap()).iter().filter(|e| e.method == "load").count()
    }

    #[test]
    fn the_same_call_within_its_ttl_is_answered_without_running() {
        let mut t = TestCells::new();
        let h = t.cell::<Users>();
        h.load(1).unwrap();
        assert_eq!(t.effects(), ["Users.load:[1]"], "keyed by the arguments");
        t.settle();
        h.load(1).unwrap();
        assert!(t.effects().is_empty(), "answered: no I/O");
        assert_eq!(runs(&h), 1, "and no event");
        h.load(2).unwrap();
        t.settle();
        assert_eq!(runs(&h), 2, "other arguments are another call");
        t.advance(59_999);
        h.load(1).unwrap();
        assert_eq!(runs(&h), 2, "still within the ttl");
        t.advance(1);
        h.load(1).unwrap();
        t.settle();
        assert_eq!(runs(&h), 3, "expired: it runs again");
        assert_eq!(h.state().names, ["user-1", "user-2", "user-1"]);
    }

    #[test]
    fn only_a_call_whose_every_part_committed_answers() {
        let mut t = TestCells::new();
        let h = t.cell::<Users>();
        h.load(1).unwrap();
        h.load(1).unwrap();
        assert_eq!(t.effects().len(), 2, "the first hasn't answered yet: both run");
        t.settle();
        h.load(0).unwrap();
        t.settle();
        h.load(0).unwrap();
        t.settle();
        assert_eq!(runs(&h), 4, "a refused step answers nothing");
        assert_eq!(t.failures().len(), 2, "both loads of user 0 were refused");
    }

    #[test]
    fn an_answer_from_before_a_clock_step_back_is_not_reused() {
        let mut t = TestCells::new();
        let h = t.cell::<Users>();
        h.load(1).unwrap();
        t.settle();
        t.advance(-1000);
        h.load(1).unwrap();
        t.settle();
        assert_eq!(runs(&h), 2);
    }

    #[test]
    fn each_step_keeps_the_key_and_the_last_one_answers() {
        let mut t = TestCells::new();
        let h = t.cell::<Users>();
        h.pair(1, 2).unwrap();
        assert_eq!(t.effects(), ["Users.pair:[1,2]"]);
        t.settle();
        let ev = h.store(|s| s.events().unwrap());
        let by: Vec<&By> = ev.iter().skip(1).map(|e| &e.by).collect();
        assert_eq!(by, [&By::Effect("pair:[1,2]".into()), &By::Effect("pair:[1,2]".into())]);
        h.pair(1, 2).unwrap();
        assert!(t.effects().is_empty(), "answered after its last step");
        assert_eq!(h.state().names, ["user-1+user-2"]);
    }

    #[test]
    fn a_rewind_forgets_the_answers() {
        let mut t = TestCells::new();
        let h = t.cell::<Users>();
        h.load(1).unwrap();
        t.settle();
        h.store(|s| s.rewind(&By::Test, 0)).unwrap();
        assert!(h.state().names.is_empty());
        h.load(1).unwrap();
        t.settle();
        assert_eq!(h.state().names, ["user-1"], "loaded again");
    }
}

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
    assert_eq!(h.checkout(String::new(), 1000).unwrap_err(), Rejected::new("nothing to buy"));
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

    let refused = h.note("no".into()).unwrap();
    assert_eq!(refused.await, Err(Rejected::new("refused after the await")), "the caller hears the refusal");
    assert_eq!(methods(&h), ["checkout", "note"]);
    assert!(h.state().notes.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_caller_awaits_the_end_of_the_method() {
    let h = Handle::<Shop>::memory();
    let end = h.checkout("book".into(), 1000).unwrap();
    assert_eq!(h.state().status, "pricing", "the call returned after its first part");
    end.await.unwrap();
    assert!(h.state().status.starts_with("done"), "settled once its last part committed");
    let over = h.checkout("television".into(), 10).unwrap().await.unwrap_err();
    assert!(over.why.starts_with("television costs"), "{over}");
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
        pub async fn name(&mut self, id: u32) -> Result<String> {
            let name: String = lookup(id).await;
            Ok(name)
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
    fn a_cached_call_answers_with_the_kept_value() {
        let mut t = TestCells::new();
        let h = t.cell::<Users>();
        let mut first = h.name(3).unwrap();
        t.settle();
        assert_eq!(first.now(), Some(Ok("user-3".into())));
        let mut again = h.name(3).unwrap();
        assert!(t.effects().is_empty(), "answered from the cache");
        assert_eq!(again.now(), Some(Ok("user-3".into())), "with the value it answered");
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

/// ST1/ST2: an async method answers its caller, and awaits anywhere a
/// statement goes (branches, matches, loops, blocks), each part still one
/// atomic, logged call.
mod anywhere {
    use aio::prelude::*;
    use std::time::Duration;

    #[cell(persist)]
    pub struct Feed {
        pub pages: Vec<String>,
        pub seen: u32,
        pub log: Vec<String>,
        pub cache: Option<u64>,
    }

    async fn page(n: u32) -> Vec<String> {
        tokio::time::sleep(Duration::from_millis(2)).await;
        if n >= 3 { vec![] } else { vec![format!("p{n}-{}", aio::uuid_v4().simple())] }
    }

    async fn lookup(id: u32) -> std::result::Result<u64, String> {
        tokio::time::sleep(Duration::from_millis(2)).await;
        if id == 0 { Err("no such id".into()) } else { Ok(u64::from(id) * 10) }
    }

    async fn tick() {}

    #[methods]
    impl Feed {
        /// A value back, a cached answer without awaiting, a `?` on the await.
        pub async fn price(&mut self, id: u32) -> Result<u64> {
            if let Some(p) = self.cache {
                return Ok(p);
            }
            let p: u64 = lookup(id).await?;
            self.cache = Some(p);
            Ok(p)
        }

        /// The last expression awaited: its `Ok` is the answer.
        pub async fn peek(&mut self, id: u32) -> Result<u64> {
            self.log.push(format!("peek {id}"));
            lookup(id).await
        }

        /// A loop with an await each turn, `break` and `continue`.
        pub async fn load(&mut self, skip: u32) -> Result<u32> {
            let mut n: u32 = 0;
            loop {
                let rows: Vec<String> = page(n).await;
                n += 1;
                if rows.is_empty() {
                    break;
                }
                if n - 1 == skip {
                    continue;
                }
                self.pages.extend(rows);
            }
            self.seen = n;
            Ok(n)
        }

        /// `while`, `match` and `if` with awaits in some arms only.
        pub async fn mixed(&mut self, mode: u32) -> Result<String> {
            let mut left: u32 = 2;
            while left > 0 {
                tick().await;
                left -= 1;
            }
            let said: String = match mode {
                0 => "zero".into(),
                1 => {
                    let p: u64 = lookup(1).await?;
                    format!("one {p}")
                }
                _ => {
                    tick().await;
                    "many".into()
                }
            };
            if mode == 7 {
                let more: u64 = lookup(7).await?;
                self.log.push(format!("{said} {more}"));
            } else {
                self.log.push(said.clone());
            }
            Ok(said)
        }
    }

    fn methods(h: &Handle<Feed>) -> Vec<String> {
        h.store(|s| s.events().unwrap()).into_iter().map(|e| e.method).collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_async_method_answers_with_its_value_or_its_refusal() {
        let h = Handle::<Feed>::memory();
        assert_eq!(h.price(4).unwrap().await, Ok(40));
        assert_eq!(methods(&h), ["price", "price.1"]);
        assert_eq!(h.price(9).unwrap().await, Ok(40), "an early return answers in the first part");
        assert_eq!(h.call("price", serde_json::json!([9])), Ok(serde_json::json!(40)), "a plain call too");
        assert_eq!(methods(&h), ["price", "price.1", "price", "price"]);

        let h = Handle::<Feed>::memory();
        assert_eq!(h.price(0).unwrap().await, Err(Rejected::new("no such id")), "an awaited `?` reaches the caller");
        assert_eq!(methods(&h), ["price"], "and nothing after it committed");
        assert_eq!(h.peek(3).unwrap().await, Ok(30));
        assert_eq!(h.peek(0).unwrap().await, Err(Rejected::new("no such id")));
        assert_eq!(h.state().log, ["peek 3", "peek 0"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn awaits_in_loops_and_branches_resume_where_they_were() {
        let h = Handle::<Feed>::memory();
        assert_eq!(h.load(1).unwrap().await, Ok(4));
        let s = h.state();
        assert_eq!(s.seen, 4);
        assert_eq!(s.pages.len(), 2, "page 1 skipped by `continue`: {:?}", s.pages);
        assert!(s.pages[0].starts_with("p0-") && s.pages[1].starts_with("p2-"));
        assert_eq!(methods(&h), ["load", "load.1", "load.1", "load.1", "load.1"], "one step per turn");

        assert_eq!(h.mixed(0).unwrap().await, Ok("zero".into()));
        assert_eq!(h.mixed(1).unwrap().await, Ok("one 10".into()));
        assert_eq!(h.mixed(2).unwrap().await, Ok("many".into()));
        assert_eq!(h.mixed(7).unwrap().await, Ok("many".into()));
        assert_eq!(h.state().log, ["zero", "one 10", "many", "many 70"]);
    }

    #[test]
    fn replay_of_loops_and_branches_is_exact_and_does_no_io() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f.db");
        let live = {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let h = Handle::<Feed>::open(&p).unwrap();
            rt.block_on(async {
                h.load(9).unwrap().await.unwrap();
                h.mixed(7).unwrap().await.unwrap();
                h.mixed(1).unwrap().await.unwrap();
            });
            h.state()
        };
        let h = Handle::<Feed>::open(&p).unwrap();
        assert_eq!(h.state(), live, "the random pages come back from the log");
    }

    #[cfg(feature = "testing")]
    #[test]
    fn under_test_cells_the_answer_is_there_once_settled() {
        let mut t = aio::testing::TestCells::new();
        let h = t.cell::<Feed>();
        let mut a = h.price(2).unwrap();
        assert_eq!(a.now(), None, "its await is held");
        t.settle();
        assert_eq!(a.now(), Some(Ok(20)));
    }
}

/// ST3/ST6: `#[mode(newest)]` and `#[mode(first)]` on async methods.
mod modes {
    use aio::prelude::*;
    use std::time::Duration;

    #[cell]
    pub struct Search {
        pub shown: Vec<String>,
        pub runs: u32,
    }

    async fn find(q: String, ms: u64) -> String {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        format!("hits for {q}")
    }

    #[methods]
    impl Search {
        #[mode(newest)]
        pub async fn query(&mut self, q: String, ms: u64) -> Result<String> {
            let hits: String = find(q, ms).await;
            self.shown.push(hits.clone());
            Ok(hits)
        }

        #[mode(first)]
        pub async fn refresh(&mut self) -> Result<u32> {
            self.runs += 1;
            let n: u32 = self.runs;
            let _ = find("all".into(), 30).await;
            Ok(n)
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn newest_supersedes_the_running_call() {
        let h = Handle::<Search>::memory();
        let slow = h.query("a".into(), 200).unwrap();
        let fast = h.query("ab".into(), 5).unwrap();
        assert_eq!(fast.await, Ok("hits for ab".into()));
        let e = slow.await.unwrap_err();
        assert!(e.why.contains("ended before it answered"), "{e}");
        assert_eq!(h.state().shown, ["hits for ab"], "the superseded call shows nothing");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn first_shares_the_running_calls_answer() {
        let h = Handle::<Search>::memory();
        let a = h.refresh().unwrap();
        let b = h.refresh().unwrap();
        assert_eq!((a.await, b.await), (Ok(1), Ok(1)), "the second caller adopts the first's answer");
        assert_eq!(h.state().runs, 1, "and ran nothing");
        assert_eq!(h.refresh().unwrap().await, Ok(2), "once it ended, a call runs again");
    }
}

/// ST1 over the wire: a synced copy's caller gets the server's end of an
/// async method (its value, or a refusal after an await), and
/// `call_confirmed` waits for that end.
#[cfg(all(feature = "server", feature = "testing"))]
mod remote {
    use aio::prelude::*;
    use aio::testing::TestServer;
    use std::time::Duration;

    #[cell(shared)]
    pub struct Orders {
        pub placed: Vec<String>,
    }

    async fn charge(item: String) -> std::result::Result<String, String> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if item == "nothing" { Err("declined".into()) } else { Ok(format!("ref-{item}")) }
    }

    #[methods]
    impl Orders {
        #[client]
        pub async fn checkout(&mut self, item: String) -> Result<String> {
            let r: String = charge(item).await?;
            self.placed.push(r.clone());
            Ok(r)
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_copy_awaits_the_servers_end() {
        let server = TestServer::start(Handle::<Orders>::memory(), Default::default()).await.unwrap();
        let ana = server.client("ana").await.unwrap();
        assert_eq!(ana.checkout("book".into()).unwrap().await, Ok("ref-book".into()));
        assert_eq!(ana.checkout("nothing".into()).unwrap().await, Err(Rejected::new("declined")));
        let a = ana.clone();
        let v = tokio::task::spawn_blocking(move || a.call_confirmed("checkout", serde_json::json!(["pen"])));
        assert_eq!(v.await.unwrap(), Ok(serde_json::json!("ref-pen")), "call_confirmed waits for the end");
        server.converged(&[&ana]).await;
        assert_eq!(ana.state().placed, ["ref-book", "ref-pen"]);
    }
}

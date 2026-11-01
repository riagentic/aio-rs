//! aio's transactional async methods (`#[cell(transaction)]`, aio's
//! `transaction: true`, docs/state/transactional-methods.md): each part of
//! the call runs on the method's own copy of the state as it was when it
//! was called, with its own writes; nothing it writes, schedules or asks
//! for is seen until its last part, which commits it all as one event or
//! nothing. Between, other calls commit as usual. At the commit, what the
//! method wrote is put on the state as it is then, unless another call
//! changed a path it wrote meanwhile (or, `serialize`, anything at all):
//! then it is refused with `TX_CONFLICT` (or, `conflict = "warn"`, said and
//! committed). aio-rs can't see what a method read, so every path it
//! wrote counts as read (aio's read-modify-write rule, never weaker).
use super::*;
use crate::answer::{Reply, Slot};
use crate::cx::Asked;
use std::collections::{HashMap, VecDeque};
use std::sync::Weak;

/// A transaction in flight: one async call's state between its parts.
pub(super) struct Tx<T> {
    /// What its reads started from: the committed state at the call, or at its last publish.
    base: Arc<T>,
    /// `base` with its own writes.
    work: T,
    /// The call is gone when nothing holds its reply (its await aborted or cut off).
    reply: Weak<Slot>,
    by: By,
    method: String,
    args: Value,
    op: Option<String>,
    /// What its parts asked for, started (or set) once it commits.
    effects: Vec<Asked>,
    plans: Vec<Plan>,
    /// When it last published (`cx.commit_every`).
    published: Option<i64>,
}

/// A serialized call waiting for the one before it to end.
pub(super) struct Parked {
    by: By,
    role: Option<String>,
    method: String,
    args: Value,
    op: Option<String>,
    reply: Reply,
}

/// The cell's transactions in flight, by call, and its serialized queue.
pub(super) struct Txs<T> {
    open: HashMap<usize, Tx<T>>,
    /// The serialized call whose turn it is.
    turn: Option<usize>,
    parked: VecDeque<Parked>,
}

impl<T> Default for Txs<T> {
    fn default() -> Self {
        Self { open: HashMap::new(), turn: None, parked: VecDeque::new() }
    }
}

/// `TX_CONFLICT`'s words for `path`, aio's.
fn conflict<T: Cell>(method: &str, path: &str, warn: bool) -> String {
    let then = match warn {
        true => "committed anyway (transaction(conflict = \"warn\"))",
        false => "nothing it wrote committed. Read cx.live_state() to work from the state now, call it again, or set #[cell(transaction(conflict = \"warn\"))] to commit anyway",
    };
    format!(
        "[{}] {method}(): {path} was changed by another call while this transaction awaited, and its reads are of the state when it was called: committing would overwrite that change with a value computed from stale state; {then}",
        T::ID
    )
}

/// `a.b` for a patch path.
fn dotted(path: &[crate::patch::Step]) -> String {
    let s: Vec<String> = path
        .iter()
        .map(|p| match p {
            crate::patch::Step::Key(k) => k.clone(),
            crate::patch::Step::Index(i) => i.to_string(),
        })
        .collect();
    format!("self.{}", s.join("."))
}

fn path_of(op: &crate::patch::Op) -> &[crate::patch::Step] {
    match op {
        crate::patch::Op::Set { path, .. }
        | crate::patch::Op::Del { path }
        | crate::patch::Op::Insert { path, .. }
        | crate::patch::Op::Append { path, .. } => path,
    }
}

/// A write that only added items at a list's end (a push): what it added.
/// It goes on the list as it is at the commit, whatever else changed in it
/// (aio: an append names no element, so it never conflicts).
fn pushed<'a>(base: &Value, op: &'a crate::patch::Op) -> Option<(&'a [crate::patch::Step], &'a [Value])> {
    let crate::patch::Op::Set { path, value: Value::Array(w) } = op else { return None };
    let Value::Array(b) = at(base, path)? else { return None };
    (w.len() > b.len() && w[..b.len()] == b[..]).then(|| (path.as_slice(), &w[b.len()..]))
}

fn at<'a>(v: &'a Value, path: &[crate::patch::Step]) -> Option<&'a Value> {
    path.iter().try_fold(v, |v, p| match p {
        crate::patch::Step::Key(k) => v.get(k),
        crate::patch::Step::Index(i) => v.get(i),
    })
}

fn at_mut<'a>(v: &'a mut Value, path: &[crate::patch::Step]) -> Option<&'a mut Value> {
    path.iter().try_fold(v, |v, p| match p {
        crate::patch::Step::Key(k) => v.get_mut(k),
        crate::patch::Step::Index(i) => v.get_mut(i),
    })
}

/// One path is the other's or inside it.
fn overlaps(a: &[crate::patch::Step], b: &[crate::patch::Step]) -> bool {
    a.iter().zip(b).all(|(x, y)| x == y)
}

/// A part of a transaction ([`Store::tx_call`]).
type Call<T> = fn(&mut Store<T>, crate::Transaction, &By, &str, Value, Cx, Reply, Option<&str>) -> Result<Value>;

/// The transaction code of cell `T`, made only for a `#[cell(transaction)]`
/// cell: every other cell's build holds none of it.
pub(crate) struct On<T>(std::marker::PhantomData<T>);

impl<T: Cell> On<T> {
    pub(super) const CALL: Option<Call<T>> = match T::TRANSACTION {
        Some(_) => Some(Store::<T>::tx_call),
        None => None,
    };
    pub(crate) const SWEEP: Option<fn(&mut Store<T>) -> bool> = match T::TRANSACTION {
        Some(_) => Some(Store::<T>::tx_sweep),
        None => None,
    };
}

impl<T: Cell> Store<T> {
    /// A part of a `#[cell(transaction)]` cell's async call (`method`,
    /// `method.N`) whose end goes to `reply` (see the module).
    #[allow(clippy::too_many_arguments)]
    fn tx_call(
        &mut self,
        cfg: crate::Transaction,
        by: &By,
        method: &str,
        args: Value,
        cx: Cx,
        reply: Reply,
        op: Option<&str>,
    ) -> Result<Value> {
        let (id, at) = (reply.id(), cx.now());
        let tx = match method.split_once('.') {
            None => {
                if cfg.serialize && self.txs.turn.is_some_and(|t| t != id) {
                    let role = cx.caller_role().map(String::from);
                    let (method, op) = (method.to_string(), op.map(String::from));
                    self.txs.parked.push_back(Parked { by: by.clone(), role, method, args, op, reply });
                    return Ok(Value::Null);
                }
                Tx {
                    base: self.state.clone(),
                    work: (*self.state).clone(),
                    reply: reply.watch(),
                    by: by.clone(),
                    method: method.into(),
                    args: args.clone(),
                    op: op.map(String::from),
                    effects: vec![],
                    plans: vec![],
                    published: None,
                }
            }
            Some(_) => match self.txs.open.remove(&id) {
                Some(tx) => tx,
                None => crate::reject!("{}.{method}: its transaction is gone", T::ID),
            },
        };
        if cfg.serialize {
            self.txs.turn = Some(id);
        }
        let out = self.tx_part(cfg, tx, method, args, cx, at, id);
        if out.is_err() || !self.txs.open.contains_key(&id) {
            self.tx_ended(id);
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn tx_part(
        &mut self,
        cfg: crate::Transaction,
        mut tx: Tx<T>,
        method: &str,
        args: Value,
        cx: Cx,
        at: i64,
        id: usize,
    ) -> Result<Value> {
        let cx = cx.with_live(self.state.clone());
        let out = match apply_guarded(&mut tx.work, &cx, method, args) {
            Err(Applied::Panicked(why)) => {
                let now = std::time::Instant::now();
                self.panics.retain(|t| now.duration_since(*t) < PANIC_WINDOW);
                self.panics.push(now);
                eprintln!("[aio] {}.{method} panicked: {why} ({} within a minute)", T::ID, self.panics.len());
                crate::reject!("{}.{method} panicked: {why}", T::ID);
            }
            Err(Applied::Refused(e)) => return Err(e),
            Ok(out) => out,
        };
        let mut asks = cx.into_asks();
        let next = asks.then.map(|i| asks.effects.remove(i));
        tx.effects.append(&mut asks.effects);
        tx.plans.append(&mut asks.plans);
        if asks.ended || next.is_none() {
            self.tx_commit(cfg, &mut tx, at)?;
            return Ok(out);
        }
        let due = asks.publish.is_some_and(|ms| tx.published.is_none_or(|p| at - p >= ms));
        if due {
            self.tx_commit(cfg, &mut tx, at)?;
            tx.published = Some(at);
        }
        self.effects.extend(next);
        self.txs.open.insert(id, tx);
        Ok(out)
    }

    /// Commit what `tx` wrote onto the state now, with what it asked for,
    /// as one event of its method (the state it left, its call's
    /// arguments); its reads go on from there.
    fn tx_commit(&mut self, cfg: crate::Transaction, tx: &mut Tx<T>, at: i64) -> Result {
        let live = self.state.clone();
        let next = match Arc::ptr_eq(&tx.base, &live) {
            true => tx.work.clone(),
            false => {
                let (base, work, mut now) = (crate::json(&*tx.base)?, crate::json(&tx.work)?, crate::json(&*live)?);
                let (pushes, mine): (Vec<_>, Vec<_>) =
                    crate::patch::diff_whole(&base, &work).into_iter().partition(|m| pushed(&base, m).is_some());
                let theirs = crate::patch::diff_whole(&base, &now);
                let clash = match cfg.serialize {
                    true => theirs.first().filter(|_| !mine.is_empty() || !pushes.is_empty()),
                    false => theirs.iter().find(|t| mine.iter().any(|m| overlaps(path_of(m), path_of(t)))),
                };
                if let Some(c) = clash {
                    let msg = conflict::<T>(&tx.method, &dotted(path_of(c)), cfg.warn);
                    if !cfg.warn {
                        return Err(Rejected::coded(crate::Code::TxConflict, msg));
                    }
                    eprintln!("[aio] {msg}");
                    crate::report::problem("tx:conflict", &msg);
                }
                let unfit = |e: String| {
                    Rejected::new(format!("{}.{}: what it wrote no longer fits the state: {e}", T::ID, tx.method))
                };
                crate::patch::apply(&mut now, &mine).map_err(unfit)?;
                for p in &pushes {
                    let (path, items) = pushed(&base, p).expect("a push");
                    match at_mut(&mut now, path).and_then(Value::as_array_mut) {
                        Some(list) => list.extend(items.iter().cloned()),
                        None => return Err(unfit(format!("{} is no longer a list", dotted(path)))),
                    }
                }
                serde_json::from_value(now)
                    .map_err(|e| Rejected::new(format!("{}.{}: what it wrote no longer fits: {e}", T::ID, tx.method)))?
            }
        };
        next.check()?;
        self.flush()?;
        let asks = Asks { plans: std::mem::take(&mut tx.plans), op: tx.op.take(), ..Asks::default() };
        let next = Arc::new(next);
        // A transaction that changed nothing and set no schedule logs nothing.
        if *next != *live || !asks.plans.is_empty() || asks.op.is_some() {
            let mut kept = crate::json(&*next)?;
            unsaved_out::<T>(&mut kept);
            let args = serde_json::json!({ TX: kept, "args": tx.args });
            self.commit(&tx.by, &tx.method, &args, &asks, at, next.clone(), None, &[])?;
        }
        self.effects.append(&mut tx.effects);
        tx.work = (*next).clone();
        tx.base = next;
        Ok(())
    }

    /// Call `id` ended (committed, refused, or gone): the next serialized
    /// call's turn.
    fn tx_ended(&mut self, id: usize) {
        self.txs.open.remove(&id);
        if self.txs.turn == Some(id) {
            self.txs.turn = None;
        }
        self.tx_next();
    }

    /// Calls whose replies nobody holds (their await aborted or cut off)
    /// dropped, with what they wrote; then the next serialized call's turn.
    /// Whether anything changed (effects may wait to be started).
    fn tx_sweep(&mut self) -> bool {
        let gone: Vec<usize> = self.txs.open.iter().filter(|(_, t)| t.reply.strong_count() == 0).map(|(i, _)| *i).collect();
        let freed = self.txs.turn.is_some_and(|t| !self.txs.open.contains_key(&t) || gone.contains(&t));
        gone.iter().for_each(|i| _ = self.txs.open.remove(i));
        if freed {
            self.txs.turn = None;
        }
        let ran = self.tx_next();
        !gone.is_empty() || ran
    }

    /// While no serialized call has the turn, start the next one parked;
    /// whether one started.
    fn tx_next(&mut self) -> bool {
        let mut ran = false;
        while self.txs.turn.is_none()
            && let Some(p) = self.txs.parked.pop_front()
        {
            ran = true;
            let r = p.reply.clone();
            if let Err(e) =
                self.call_as(&p.by, p.role.as_deref(), &p.method, p.args, p.op.as_deref(), Some(p.reply))
            {
                r.send(Err(e));
            }
        }
        ran
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::Step;

    #[test]
    fn a_path_overlaps_its_own_inside_and_outside_never_a_sibling() {
        let (a, ab, ac) = ([Step::Key("a".into())], [Step::Key("a".into()), Step::Key("b".into())], [
            Step::Key("a".into()),
            Step::Key("c".into()),
        ]);
        assert!(overlaps(&a, &ab) && overlaps(&ab, &a) && overlaps(&ab, &ab));
        assert!(!overlaps(&ab, &ac));
        assert_eq!(dotted(&[Step::Key("items".into()), Step::Index(0)]), "self.items.0");
    }
}

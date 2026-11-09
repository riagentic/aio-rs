//! The Redux DevTools bridge (aio's `connectReduxDevTools`): in a dev build
//! each `client!` cell is an instance of the browser extension, named by
//! its id. Each commit is an action (the views' calls with their arguments,
//! else the store's methods; `@@aio/state` when neither says) with the
//! state after it. The extension's jumps are taken: a store's cell travels
//! (`TimeAsk::Goto`, paused until the newest action is jumped to), a
//! page's own cell is put back to the state the extension holds. Its
//! dispatcher calls a method (`{"type": "add", "args": [1]}`), its pause
//! stops the sending, its commit, revert and reset work. Where the page has
//! no extension nothing is sent. A release build carries none of it.
use crate::ui::prelude::*;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::rc::Rc;

/// A time-travel step in the store's log: never an action of its own.
const REWIND: &str = "$rewind";

/// Where a store's cell stands and its newest events (`seq`, method).
#[doc(hidden)]
pub type Steps = Option<(u64, Vec<(u64, String)>)>;

/// A view's call: its cell, method and arguments.
type Call = (&'static str, String, Value);

/// A `client!` cell as the bridge sees it (its `use_live` says so).
#[derive(Clone, Copy)]
#[doc(hidden)]
pub struct Bridged {
    pub cell: &'static str,
    /// The state as JSON; read in an effect, it follows every commit.
    pub state: fn() -> Value,
    /// `None` for a page's or a synced copy's (no history here).
    pub steps: fn() -> Steps,
    /// To the state after event `seq` (a store's), or to `state` (a page's
    /// own); `newest`: calls are taken again.
    pub jump: fn(Option<u64>, &Value, bool) -> Result<(), String>,
    /// The views' call of `method` with `args`.
    pub dispatch: fn(&str, Value) -> Result<(), String>,
}

/// This app instance's bridged cells and the views' calls not yet told.
#[derive(Clone, Default)]
struct Said(Rc<RefCell<(Vec<Bridged>, Vec<Call>)>>);

static SAID: Global<Said> = Global::named(Said::default, "aio::kit::redux::SAID");

/// `client!`'s `use_live()` in a dev build: its cell is bridged.
#[doc(hidden)]
pub fn register(b: Bridged) {
    let said = SAID.get_untracked();
    let mut s = said.0.borrow_mut();
    if !s.0.iter().any(|x| x.cell == b.cell) {
        s.0.push(b);
    }
}

/// `client!`'s door in a dev build: a call of the views that ran, told
/// with the commit it made.
#[doc(hidden)]
pub fn called(cell: &'static str, method: &str, args: Value) {
    SAID.get_untracked().0.borrow_mut().1.push((cell, method.into(), args));
}

/// A store cell's where-and-steps, for [`Bridged::steps`].
#[doc(hidden)]
pub fn steps<T: crate::Cell>(s: &crate::Source<T>) -> Steps {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let t = s.travel(None).ok()?;
        Some((t.at, t.steps.into_iter().map(|m| (m.seq, m.method)).collect()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        None
    }
}

/// A jump, for [`Bridged::jump`]: a store's cell travels to `seq`, resumed
/// at the newest; a page's own cell takes `state`.
#[doc(hidden)]
pub fn jump<T: crate::Cell>(s: &crate::Source<T>, seq: Option<u64>, state: &Value, newest: bool) -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = state;
        let seq = seq.ok_or_else(|| format!("{}: no event of the store to go to", T::ID))?;
        s.travel(Some(crate::TimeAsk::Goto(seq))).map_err(|e| e.why)?;
        if newest {
            s.travel(Some(crate::TimeAsk::Resume)).map_err(|e| e.why)?;
        }
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (seq, newest);
        s.put(state)
    }
}

/// What the extension asked, once read.
#[derive(Debug, PartialEq)]
enum Do {
    Nothing,
    /// Go to an action's state; `rebase`: it is the new start (revert, reset).
    Jump {
        seq: Option<u64>,
        state: Value,
        newest: bool,
        rebase: bool,
    },
    /// Start the history again from the state now (commit).
    Rebase,
    Call(String, Value),
}

/// One instance's side of the protocol (pure): the store event each
/// action the extension holds stands for, where the store was last seen,
/// the state it was last told or jumped to, whether it records.
#[derive(Debug, Default)]
struct Link {
    ids: Vec<(Option<u64>, Value)>,
    newest: u64,
    told: Value,
    paused: bool,
}

impl Link {
    /// The extension's start: the state now (`init`).
    fn init(&mut self, state: Value, steps: Steps) -> Value {
        let at = steps.as_ref().map(|(at, _)| *at);
        self.newest = steps.and_then(|(_, s)| s.last().map(|m| m.0)).unwrap_or(0);
        self.ids = vec![(at, state.clone())];
        self.told = state.clone();
        json!({ "init": state })
    }

    /// A commit: its action and state for the extension, or `None` (no
    /// change since told, time travel's own step, recording paused).
    fn commit(&mut self, state: Value, calls: Vec<(String, Value)>, steps: Steps) -> Option<Value> {
        let at = steps.as_ref().map(|(at, _)| *at);
        let after = self.newest;
        let new: Vec<String> = steps
            .into_iter()
            .flat_map(|(_, s)| s)
            .filter(|(seq, _)| *seq > after)
            .map(|(seq, m)| {
                self.newest = seq;
                m
            })
            .collect();
        if state == self.told || (!new.is_empty() && new.iter().all(|m| m == REWIND)) {
            self.told = state;
            return None;
        }
        self.told = state.clone();
        if self.paused {
            return None;
        }
        self.ids.push((at, state.clone()));
        Some(json!({ "action": action(calls, &new), "state": state }))
    }

    /// What the extension's message `m` asks.
    fn asked(&mut self, m: &Value) -> Do {
        let to = |l: &mut Self, id: usize, state: Option<Value>, rebase: bool| match l.ids.get(id).cloned() {
            Some((seq, kept)) => {
                let state = state.unwrap_or(kept);
                l.told = state.clone();
                Do::Jump { seq, state, newest: id + 1 == l.ids.len(), rebase }
            }
            None => Do::Nothing,
        };
        let id = m["payload"]["actionId"].as_u64().and_then(|n| usize::try_from(n).ok());
        match (m["type"].as_str(), m["payload"]["type"].as_str()) {
            (Some("DISPATCH"), Some("JUMP_TO_STATE" | "JUMP_TO_ACTION")) => match id {
                Some(id) => to(self, id, m["state"].as_str().and_then(|s| serde_json::from_str(s).ok()), false),
                None => Do::Nothing,
            },
            (Some("DISPATCH"), Some("PAUSE_RECORDING")) => {
                self.paused = m["payload"]["status"].as_bool().unwrap_or(!self.paused);
                Do::Nothing
            }
            (Some("DISPATCH"), Some("COMMIT")) => Do::Rebase,
            (Some("DISPATCH"), Some("ROLLBACK" | "RESET")) => to(self, 0, None, true),
            (Some("ACTION"), _) => dispatched(&m["payload"]),
            _ => Do::Nothing,
        }
    }
}

/// An action as the extension shows it: the views' calls (one: its
/// method and arguments; more: each), else the store's methods, else
/// `@@aio/state` (pure).
fn action(calls: Vec<(String, Value)>, steps: &[String]) -> Value {
    match (calls.len(), steps.is_empty()) {
        (0, true) => json!({ "type": "@@aio/state" }),
        (0, false) => json!({ "type": steps.join(" + ") }),
        (1, _) => {
            let (m, a) = calls.into_iter().next().unwrap_or_default();
            json!({ "type": m, "args": a })
        }
        _ => {
            let kind = calls.iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>().join(" + ");
            let each: Vec<Value> = calls.into_iter().map(|(m, a)| json!({ "type": m, "args": a })).collect();
            json!({ "type": kind, "calls": each })
        }
    }
}

/// The dispatcher's text: `{"type": method, "args": [...]}` or a bare
/// method name (pure). The page allows no `eval`: JSON only.
fn dispatched(p: &Value) -> Do {
    let v = match p {
        Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({ "type": s.trim() })),
        v => v.clone(),
    };
    match v["type"].as_str() {
        Some(m) if !m.is_empty() => Do::Call(m.into(), v.get("args").cloned().unwrap_or(Value::Null)),
        _ => Do::Nothing,
    }
}

/// The page's side: the extension's instance named `name`, kept on the
/// page across a hot reload (its old listeners dropped); `false` first
/// when the page has no extension, else `true`, then what it is told.
fn script(name: &str) -> String {
    format!(
        "const ext = window.__REDUX_DEVTOOLS_EXTENSION__;
        if (!ext || typeof ext.connect !== 'function') {{ aio.send(false); return; }}
        const all = window.__aioRedux || (window.__aioRedux = {{}}), name = {name};
        const c = all[name] || (all[name] = ext.connect({{ name, features: {{ pause: true, lock: false, persist: false,
          export: true, import: false, jump: true, skip: false, reorder: false, dispatch: true, test: false }} }}));
        if (c.unsubscribe) c.unsubscribe();
        c.subscribe((m) => aio.send(m));
        aio.send(true);
        for (;;) {{
          const m = await aio.recv();
          if ('init' in m) c.init(m.init); else c.send(m.action, m.state);
        }}",
        name = Value::from(name)
    )
}

/// What a dev build mounts beside the app (`aio_shell`'s window, a web
/// build's page): each `client!` cell in the Redux DevTools extension, as
/// its cells go live.
#[component]
pub fn ReduxDevTools() -> View {
    let cells = signal(|| 0usize);
    spawn(async move {
        loop {
            let n = SAID.get_untracked().0.borrow().0.len();
            if n != cells.get_untracked() {
                cells.set(n);
            }
            crate::ui::clock::sleep(std::time::Duration::from_millis(250)).await;
        }
    });
    rsx! {
        for at in 0..cells.get() {
            Instance { key: at, at }
        }
    }
}

/// The `at`th bridged cell's instance.
#[component]
fn Instance(at: usize) -> View {
    let b = SAID.get_untracked().0.borrow().0[at];
    let page = eval(&script(b.cell));
    let link = Rc::new(RefCell::new(Link::default()));
    let on = signal(|| false);
    {
        let (page, link) = (page.clone(), link.clone());
        effect(move || {
            let state = (b.state)();
            if !on.get() {
                return;
            }
            untrack(|| {
                let said = SAID.get_untracked();
                let mut s = said.0.borrow_mut();
                let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut s.1).into_iter().partition(|c| c.0 == b.cell);
                s.1 = rest;
                drop(s);
                let calls = mine.into_iter().map(|(_, m, a)| (m, a)).collect();
                if let Some(m) = link.borrow_mut().commit(state, calls, (b.steps)()) {
                    _ = page.send(m);
                }
            });
        });
    }
    spawn(async move {
        let mut page = page;
        if !matches!(page.recv::<bool>().await, Ok(true)) {
            return;
        }
        let first = link.borrow_mut().init((b.state)(), (b.steps)());
        _ = page.send(first);
        on.set(true);
        while let Ok(m) = page.recv::<Value>().await {
            let done = match link.borrow_mut().asked(&m) {
                Do::Nothing => Ok(false),
                Do::Rebase => Ok(true),
                Do::Jump { seq, state, newest, rebase } => (b.jump)(seq, &state, newest).map(|_| rebase),
                Do::Call(method, args) => (b.dispatch)(&method, args).map(|_| false),
            };
            match done {
                Ok(true) => {
                    let again = link.borrow_mut().init((b.state)(), (b.steps)());
                    _ = page.send(again);
                }
                Ok(false) => {}
                Err(e) => crate::warn!(cat: "ui", "Redux DevTools on {}: {e}", b.cell),
            }
        }
    });
    rsx! {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(at: u64, s: &[(u64, &str)]) -> Steps {
        Some((at, s.iter().map(|(n, m)| (*n, m.to_string())).collect()))
    }

    #[test]
    fn a_commit_is_an_action_named_by_the_views_calls_else_the_stores_steps() {
        let mut l = Link::default();
        assert_eq!(l.init(json!({ "n": 0 }), steps(0, &[])), json!({ "init": { "n": 0 } }));
        let calls = vec![("add".to_string(), json!([2]))];
        assert_eq!(
            l.commit(json!({ "n": 2 }), calls, steps(1, &[(1, "add")])),
            Some(json!({ "action": { "type": "add", "args": [2] }, "state": { "n": 2 } }))
        );
        // An effect's or a schedule's commit: the store names it.
        assert_eq!(
            l.commit(json!({ "n": 3 }), vec![], steps(3, &[(1, "add"), (2, "tick"), (3, "tick")]))
                .map(|m| m["action"].clone()),
            Some(json!({ "type": "tick + tick" }))
        );
        // A page's own cell has no store: its state alone.
        let mut page = Link::default();
        page.init(json!(0), None);
        assert_eq!(
            page.commit(json!(1), vec![], None).map(|m| m["action"].clone()),
            Some(json!({ "type": "@@aio/state" }))
        );
        let two = vec![("a".to_string(), Value::Null), ("b".to_string(), json!([1]))];
        assert_eq!(page.commit(json!(2), two, None).unwrap()["action"]["type"], "a + b");
        assert_eq!(page.ids.len(), 3, "the start and two actions");
    }

    #[test]
    fn time_travels_own_steps_and_unchanged_states_are_no_actions() {
        let mut l = Link::default();
        l.init(json!(1), steps(1, &[(1, "add")]));
        assert_eq!(l.commit(json!(1), vec![], steps(1, &[(1, "add")])), None, "nothing changed");
        assert_eq!(l.commit(json!(0), vec![], steps(0, &[(1, "add"), (2, REWIND)])), None, "a rewind");
        assert!(l.commit(json!(5), vec![], steps(3, &[(2, REWIND), (3, "add")])).is_some(), "past it, calls count");
        l.asked(&json!({ "type": "DISPATCH", "payload": { "type": "PAUSE_RECORDING", "status": true } }));
        assert_eq!(l.commit(json!(6), vec![], steps(4, &[(4, "add")])), None, "paused: not recorded");
        l.asked(&json!({ "type": "DISPATCH", "payload": { "type": "PAUSE_RECORDING", "status": false } }));
        assert!(l.commit(json!(7), vec![], steps(5, &[(5, "add")])).is_some());
    }

    #[test]
    fn a_jump_goes_to_the_actions_store_event_or_its_state_and_resumes_at_the_newest() {
        let mut l = Link::default();
        l.init(json!({ "n": 0 }), steps(0, &[]));
        l.commit(json!({ "n": 1 }), vec![], steps(1, &[(1, "add")]));
        l.commit(json!({ "n": 2 }), vec![], steps(2, &[(2, "add")]));
        let jump = |id: u64, state: &str| json!({ "type": "DISPATCH", "payload": { "type": "JUMP_TO_ACTION", "actionId": id }, "state": state });
        assert_eq!(
            l.asked(&jump(1, r#"{"n":1}"#)),
            Do::Jump { seq: Some(1), state: json!({ "n": 1 }), newest: false, rebase: false }
        );
        assert_eq!(l.commit(json!({ "n": 1 }), vec![], steps(1, &[(3, REWIND)])), None, "the jump's own step");
        assert!(matches!(l.asked(&jump(2, r#"{"n":2}"#)), Do::Jump { seq: Some(2), newest: true, .. }));
        assert_eq!(l.asked(&jump(9, "{}")), Do::Nothing, "an action it never had");
        let reset = json!({ "type": "DISPATCH", "payload": { "type": "RESET" } });
        assert_eq!(l.asked(&reset), Do::Jump { seq: Some(0), state: json!({ "n": 0 }), newest: false, rebase: true });
        assert_eq!(l.asked(&json!({ "type": "DISPATCH", "payload": { "type": "COMMIT" } })), Do::Rebase);
        // A page's own cell: the state the extension holds.
        let mut page = Link::default();
        page.init(json!(0), None);
        page.commit(json!(1), vec![], None);
        assert_eq!(page.asked(&jump(0, "0")), Do::Jump { seq: None, state: json!(0), newest: false, rebase: false });
        assert_eq!(page.commit(json!(0), vec![], None), None, "the state jumped to is no action");
    }

    #[test]
    fn the_dispatcher_calls_a_method_by_json_or_name() {
        let mut l = Link::default();
        let act = |p: &str| json!({ "type": "ACTION", "payload": p });
        assert_eq!(l.asked(&act(r#"{"type":"add","args":[10]}"#)), Do::Call("add".into(), json!([10])));
        assert_eq!(l.asked(&act(" clear ")), Do::Call("clear".into(), Value::Null));
        assert_eq!(l.asked(&act("")), Do::Nothing);
        assert_eq!(l.asked(&json!({ "type": "START" })), Do::Nothing);
    }

    #[test]
    fn the_page_script_names_its_instance_and_says_when_there_is_no_extension() {
        let js = script("notes\"x");
        assert!(js.contains(r#"name = "notes\"x""#) && js.contains("aio.send(false)") && js.contains("jump: true"));
    }
}

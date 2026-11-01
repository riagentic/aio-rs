//! Verbs that talk to a running app over its local door (`aio::sync::Live`):
//! its live state, calls to its methods, its open links. Nothing is queued:
//! when the app isn't there, a call says so and was never sent.
use aio::serde_json::{self, Value};
use aio::sync::{Link, Live};

type Out = Result<String, String>;

/// The path grammar, said on a path that can't be read.
const SYNTAX: &str =
    "a path: a.b · a.0 or a[0] · a.* or a[*] · a.{x,y} · a[?k] (k truthy) · a[?k=v] · a[?k!=v] (v as JSON, else text)";

/// One step of a path.
#[derive(Debug, PartialEq)]
enum Step {
    Key(String),
    Index(usize),
    All,
    /// `[?k]`, `[?k=v]`, `[?k!=v]`: the list's items that pass.
    Filter(String, Option<(bool, Value)>),
    /// `{a,b}`: those fields (each may be a path) of a map.
    Pick(Vec<String>),
}

impl std::fmt::Display for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Step::Key(k) => write!(f, "{k}"),
            Step::Index(i) => write!(f, "[{i}]"),
            Step::All => write!(f, "*"),
            Step::Filter(k, None) => write!(f, "[?{k}]"),
            Step::Filter(k, Some((eq, v))) => write!(f, "[?{k}{}{v}]", if *eq { "=" } else { "!=" }),
            Step::Pick(ks) => write!(f, "{{{}}}", ks.join(",")),
        }
    }
}

/// `path` as steps; `Err` names what can't be read (pure).
fn steps(path: &str) -> Result<Vec<Step>, String> {
    let mut out = Vec::new();
    let mut key = String::new();
    let flush = |key: &mut String, out: &mut Vec<Step>| match std::mem::take(key) {
        k if k == "*" => out.push(Step::All),
        k if !k.is_empty() => out.push(Step::Key(k)),
        _ => {}
    };
    let mut chars = path.chars();
    while let Some(c) = chars.next() {
        match c {
            '.' => flush(&mut key, &mut out),
            '[' => {
                flush(&mut key, &mut out);
                // To the `]` that closes it: `[?tags=["x"]]` holds.
                let (mut depth, mut inner) = (1, String::new());
                for c in chars.by_ref() {
                    depth += match c {
                        '[' => 1,
                        ']' => -1,
                        _ => 0,
                    };
                    if depth == 0 {
                        break;
                    }
                    inner.push(c);
                }
                if depth != 0 {
                    return Err(format!("`{path}`: a `[` is never closed ({SYNTAX})"));
                }
                out.push(match inner.as_str() {
                    "*" => Step::All,
                    i if i.parse::<usize>().is_ok() => Step::Index(i.parse().unwrap_or_default()),
                    f if f.starts_with('?') && f.len() > 1 => filter(&f[1..]),
                    other => return Err(format!("`[{other}]` in `{path}` is no step ({SYNTAX})")),
                });
            }
            '{' => {
                flush(&mut key, &mut out);
                let inner: String = chars.by_ref().take_while(|c| *c != '}').collect();
                let keys: Vec<String> =
                    inner.split(',').map(str::trim).filter(|k| !k.is_empty()).map(String::from).collect();
                if keys.is_empty() {
                    return Err(format!("`{{{inner}}}` in `{path}` picks nothing ({SYNTAX})"));
                }
                out.push(Step::Pick(keys));
            }
            c => key.push(c),
        }
    }
    flush(&mut key, &mut out);
    Ok(out)
}

/// `k`, `k=v` or `k!=v`; `v` is JSON when it reads as JSON, else text.
fn filter(expr: &str) -> Step {
    let (key, test) = match (expr.find("!="), expr.find('=')) {
        (Some(i), _) => (&expr[..i], Some((false, &expr[i + 2..]))),
        (None, Some(i)) => (&expr[..i], Some((true, &expr[i + 1..]))),
        _ => (expr, None),
    };
    let value = |v: &str| serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.into()));
    Step::Filter(key.trim().into(), test.map(|(eq, v)| (eq, value(v))))
}

/// JavaScript's truthiness, which aio's `[?k]` had: not null, false, 0 or "".
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// The value at `path` in `v` ([`SYNTAX`]); a miss says how far the path got.
pub(crate) fn pick(v: &Value, path: &str) -> Result<Value, String> {
    walk(v, &steps(path)?, &mut Vec::new())
}

fn walk(at: &Value, steps: &[Step], done: &mut Vec<String>) -> Result<Value, String> {
    let Some((step, rest)) = steps.split_first() else { return Ok(at.clone()) };
    let there = |done: &[String]| if done.is_empty() { "the state".into() } else { format!("`{}`", done.join(".")) };
    let over = |items: &[Value], done: &mut Vec<String>| -> Result<Value, String> {
        items.iter().map(|i| walk(i, rest, &mut done.clone())).collect::<Result<_, _>>().map(Value::Array)
    };
    let next = match (step, at) {
        (Step::All, Value::Array(a)) => return over(a, done),
        (Step::All, Value::Object(o)) => return over(&o.values().cloned().collect::<Vec<_>>(), done),
        (Step::All, _) => return Err(format!("{} is not a list or a map: `*` has nothing to go over", there(done))),
        (Step::Key(k), Value::Array(a)) => k.parse::<usize>().ok().and_then(|i| a.get(i)).cloned(),
        (Step::Key(k), Value::Object(o)) => o.get(k).cloned(),
        (Step::Index(i), Value::Array(a)) => a.get(*i).cloned(),
        (Step::Filter(k, test), Value::Array(a)) => {
            let has: Vec<Option<Value>> = a.iter().map(|i| pick(i, k).ok()).collect();
            if !a.is_empty() && has.iter().all(Option::is_none) {
                return Err(format!("no item of {} has `{k}` (a typo is never a match)", there(done)));
            }
            let kept: Vec<Value> = a
                .iter()
                .zip(&has)
                .filter(|(_, v)| match (test, v) {
                    (None, v) => v.as_ref().is_some_and(truthy),
                    (Some((eq, want)), v) => (v.as_ref() == Some(want)) == *eq,
                })
                .map(|(i, _)| i.clone())
                .collect();
            done.push(step.to_string());
            // A bracket after it addresses the kept list; anything else is
            // asked of each item, as after `*`.
            return match rest.first() {
                None | Some(Step::Index(_) | Step::Filter(..)) => walk(&Value::Array(kept), rest, done),
                Some(_) => over(&kept, done),
            };
        }
        (Step::Pick(keys), Value::Object(_)) => {
            let got: serde_json::Map<String, Value> =
                keys.iter().filter_map(|k| pick(at, k).ok().map(|v| (k.clone(), v))).collect();
            (!got.is_empty()).then_some(Value::Object(got))
        }
        _ => None,
    };
    let Some(next) = next else {
        let keys = match at {
            Value::Object(o) => format!(": it has {}", o.keys().cloned().collect::<Vec<_>>().join(", ")),
            Value::Array(a) => format!(": a list of {}", a.len()),
            _ => String::new(),
        };
        return Err(format!("no `{step}` in {}{keys}", there(done)));
    };
    done.push(step.to_string());
    walk(&next, rest, done)
}

/// A call's arguments: `--args` exactly (a JSON list; `@file` reads it from
/// a file, `-` from stdin), else each word as JSON when it is (`3`, `true`,
/// `{"a":1}`), as text when not; words that are all `key=value` (aio's
/// named payload) one object of them, each value read as a word. None is
/// `null`, what a method without parameters takes.
pub(crate) fn call_args(words: &[&str], exact: Option<&str>) -> Result<Value, String> {
    let list = match exact {
        Some(_) if !words.is_empty() => return Err("give the arguments as words or as --args, not both".into()),
        Some(text) => {
            let text = match text {
                "-" => std::io::read_to_string(std::io::stdin()).map_err(|e| format!("--args -: {e}"))?,
                t => match t.strip_prefix('@') {
                    Some(f) => std::fs::read_to_string(f).map_err(|e| format!("--args @{f}: {e}"))?,
                    None => t.to_string(),
                },
            };
            match serde_json::from_str::<Value>(&text) {
                Ok(Value::Array(a)) => a,
                _ => return Err(format!("--args takes a JSON list, like --args '[\"text\", 3]'; got {text}")),
            }
        }
        None if !words.is_empty() && words.iter().all(|w| pair(w).is_some()) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in words.iter().filter_map(|w| pair(w)) {
                if obj.insert(k.into(), word(v)?).is_some() {
                    return Err(format!("{k}= is given twice"));
                }
            }
            vec![Value::Object(obj)]
        }
        None => words.iter().map(|w| word(w)).collect::<Result<_, _>>()?,
    };
    Ok(if list.is_empty() { Value::Null } else { Value::Array(list) })
}

/// `key=value`, its key a name (`title=x`; not `https://x/?a=b`).
fn pair(w: &str) -> Option<(&str, &str)> {
    let (k, v) = w.split_once('=')?;
    let name = k.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    name.then_some((k, v))
}

/// `--body '{..}'`: that one object as the call's one argument.
pub(crate) fn body(text: &str) -> Result<Value, String> {
    match serde_json::from_str::<Value>(text) {
        Ok(v @ Value::Object(_)) => Ok(Value::Array(vec![v])),
        _ => Err(format!("--body takes one JSON object, like --body '{{\"title\": \"x\"}}'; got {text}")),
    }
}

/// One word: its JSON value, or the word as text. A whole number too big
/// for JSON would arrive changed, so it is refused instead.
fn word(w: &str) -> Result<Value, String> {
    match serde_json::from_str::<Value>(w) {
        Ok(Value::Number(n)) if n.is_f64() && !w.contains(['.', 'e', 'E']) => {
            Err(format!("{w} doesn't fit a JSON number exactly: pass it as text, --args '[\"{w}\"]'"))
        }
        Ok(v) => Ok(v),
        Err(_) => Ok(Value::String(w.into())),
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

/// The state, or the value at `path`, under `head` (left out for a path:
/// a script reads the value alone).
pub(crate) fn show(head: &str, v: &Value, path: Option<&str>) -> Out {
    match path {
        Some(p) => pick(v, p).map(|v| pretty(&v)),
        None => Ok(format!("{head}\n{}", pretty(v))),
    }
}

fn connect(link: &Link, cell: &str, user: &str) -> std::io::Result<Live> {
    aio::rt().block_on(Live::connect(link, cell, user))
}

/// The live state as `user` sees it; `Err` when the door can't be reached.
pub(crate) fn state(link: &Link, cell: &str, path: Option<&str>, user: &str) -> std::io::Result<Out> {
    let t = connect(link, cell, user)?;
    Ok(show(&format!("{cell} v{} at event {} (live)", t.version(), t.seq()), t.view(), path))
}

/// Print the state (or the value at `path`) on one line now and after each
/// change to it, until the app closes the link.
pub(crate) fn watch(link: &Link, cell: &str, path: Option<&str>, user: &str) -> Out {
    let mut t = connect(link, cell, user).map_err(|e| e.to_string())?;
    let mut last = None;
    loop {
        let now = match path {
            Some(p) => pick(t.view(), p)?,
            None => t.view().clone(),
        };
        if last.as_ref() != Some(&now) {
            println!("{now}");
            last = Some(now);
        }
        aio::rt().block_on(t.changed()).map_err(|e| e.to_string())?;
    }
}

pub(crate) const OPS: [&str; 9] = ["eq", "ne", "gt", "gte", "lt", "lte", "contains", "exists", "absent"];

/// The ops' words checked (`exists`/`absent` take no value, the others
/// one), before anything is asked of the app.
pub(crate) fn expect_usage(op: &str, value: Option<&str>) -> Result<(), String> {
    let unary = matches!(op, "exists" | "absent");
    match () {
        _ if !OPS.contains(&op) => Err(format!("unknown op `{op}` (use: {})", OPS.join(" "))),
        _ if unary && value.is_some() => Err(format!("{op} takes no value")),
        _ if !unary && value.is_none() => Err(format!("{op} needs a value to compare against, e.g. {op} 3")),
        _ => Ok(()),
    }
}

/// Equal as JSON means: 1 and 1.0 alike, maps in any key order.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// Does `actual` (`None`: the path isn't there) pass `op` against `raw` (the
/// value as typed: JSON, else text)? Pure; the reason says why either way.
pub(crate) fn compare(actual: Option<&Value>, op: &str, raw: Option<&str>) -> (bool, String) {
    let want = raw.map(|r| serde_json::from_str(r).unwrap_or_else(|_| Value::String(r.into())));
    let j = |v: &Value| v.to_string();
    let Some(a) = actual else {
        // A missing path has no value: only `absent` passes on it (aio: a typo
        // must never pass an `ne`).
        return match op {
            "absent" => (true, "absent".into()),
            _ => (false, "path not found: no value to compare (`absent` asserts a path is missing)".into()),
        };
    };
    let order = |sym: &str, cmp: fn(f64, f64) -> bool| match (a.as_f64(), want.as_ref().and_then(Value::as_f64)) {
        (Some(x), Some(y)) => (cmp(x, y), format!("{} {sym} {}", j(a), raw.unwrap_or(""))),
        (None, _) => (false, format!("{sym} needs numbers: actual {} is not one", j(a))),
        (_, None) => (false, format!("{sym} needs numbers: expected {} is not one", raw.unwrap_or(""))),
    };
    let w = want.clone().unwrap_or(Value::Null);
    match op {
        "exists" => (true, "present".into()),
        "absent" => (false, format!("present ({})", j(a))),
        "eq" => (same(a, &w), format!("{} vs {}", j(a), j(&w))),
        "ne" => (!same(a, &w), format!("{} vs {}", j(a), j(&w))),
        "gt" => order(">", |x, y| x > y),
        "gte" => order(">=", |x, y| x >= y),
        "lt" => order("<", |x, y| x < y),
        "lte" => order("<=", |x, y| x <= y),
        // A substring is of the characters typed (`1.50`, not 1.5).
        "contains" => match a {
            Value::String(s) => {
                let needle = w.as_str().map_or(raw.unwrap_or(""), |n| n);
                (s.contains(needle), format!("{} ⊇ {}", j(a), j(&w)))
            }
            Value::Array(items) => (items.iter().any(|x| same(x, &w)), format!("{} ∋ {}", j(a), j(&w))),
            _ => (false, format!("{} is not a text or a list", j(a))),
        },
        _ => (false, format!("unknown op `{op}` (use: {})", OPS.join(" "))),
    }
}

/// Assert on the live state as `user`: `path op [value]`, `Ok` PASS or
/// `Err` FAIL. With `wait`, the state is checked again after each change
/// (pushed, not polled) until it passes or the time is up.
pub(crate) fn expect(
    link: &Link,
    cell: &str,
    (path, op, raw): (&str, &str, Option<&str>),
    wait: std::time::Duration,
    user: &str,
) -> Out {
    expect_usage(op, raw)?;
    let mut t = connect(link, cell, user).map_err(|e| e.to_string())?;
    let line = format!("{path} {op}{}", raw.map_or(String::new(), |r| format!(" {r}")));
    let end = std::time::Instant::now() + wait;
    loop {
        let (ok, why) = compare(pick(t.view(), path).ok().as_ref(), op, raw);
        if ok {
            return Ok(format!("PASS  {line}"));
        }
        let left = end.saturating_duration_since(std::time::Instant::now());
        let changed = aio::rt().block_on(async { tokio::time::timeout(left, t.changed()).await });
        match changed {
            Ok(Ok(_)) if !left.is_zero() => continue,
            Ok(Err(e)) => return Err(format!("FAIL  {line}: {why} (then the app closed the link: {e})")),
            _ => return Err(format!("FAIL  {line}: {why}")),
        }
    }
}

/// Call `method` with `args`, as `user`; the method's answer.
pub(crate) fn call(link: &Link, cell: &str, method: &str, args: Value, user: &str) -> Out {
    call_within(link, cell, method, args, user, None)
}

/// [`call`], its answer waited for `wait` at most (`--wait`).
pub(crate) fn call_within(
    link: &Link,
    cell: &str,
    method: &str,
    args: Value,
    user: &str,
    wait: Option<std::time::Duration>,
) -> Out {
    let mut t = connect(link, cell, user).map_err(|e| e.to_string())?;
    let r = aio::rt().block_on(async {
        match wait {
            Some(w) => tokio::time::timeout(w, t.call(method, args)).await.map_err(|_| {
                format!("no answer within {} s (it may still run in the app: ax events says)", w.as_secs_f64())
            }),
            None => Ok(t.call(method, args).await),
        }
    })?;
    Ok(pretty(&r.map_err(|e| format!("refused: {e}"))?))
}

/// `ax peer <app>` (aio: `am peer`): what a network caller may call of each
/// cell of the running app, `cells` as its door answers (`peer`), a line a
/// method with its signature and the answer of its rule (pure).
pub(crate) fn peer(cells: &[Value], json: bool) -> String {
    if json {
        return Value::Array(cells.to_vec()).to_string();
    }
    let mut out = vec![];
    for c in cells {
        let cell = c["cell"].as_str().unwrap_or("?");
        out.push(format!("{cell} v{}", c["v"]));
        let f = &c["fields"];
        let list = |k: &str| f[k].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "));
        match (f["persist"].as_bool(), list("server"), list("transient")) {
            (Some(false), ..) => out.push("  fields: kept in memory, never saved".into()),
            (Some(true), s, t) => {
                let said: Vec<String> = [("never sent", s), ("never saved", t)]
                    .into_iter()
                    .filter_map(|(how, l)| l.filter(|l| !l.is_empty()).map(|l| format!("{l} {how}")))
                    .collect();
                if !said.is_empty() {
                    out.push(format!("  fields: {}", said.join("; ")));
                }
            }
            _ => {}
        }
        for m in c["methods"].as_array().into_iter().flatten() {
            let sig = m["sig"].as_str().or(m["method"].as_str()).unwrap_or("?");
            out.push(format!("  {cell}.{sig:<40}  {}", m["may"].as_str().unwrap_or("?")));
        }
    }
    if out.is_empty() { "no cell answers on the running app's door".into() } else { out.join("\n") }
}

/// `ax peer --types`: what `cells` (as [`peer`] reads them) let a caller
/// call, as Rust (pure): a module a cell, its `ID` and a `&str` const a
/// method under its signature, so a peer's call by name breaks at compile
/// time when the method is renamed (aio's peer types).
pub(crate) fn peer_types(app: &str, cells: &[Value]) -> String {
    let mut out = format!(
        "//! Written by `ax peer {app} --types` from what {app} answered: its cells and\n\
         //! methods by name, for `aio::peer::Peer::call`. Run it again when {app} changes.\n\
         #![allow(dead_code, non_upper_case_globals)]\n"
    );
    for c in cells {
        let cell = c["cell"].as_str().unwrap_or("?");
        out.push_str(&format!(
            "\n/// {cell} v{}\npub mod {} {{\n    pub const ID: &str = {cell:?};\n",
            c["v"],
            crate::testgen::ident(cell)
        ));
        for m in c["methods"].as_array().into_iter().flatten() {
            let name = m["method"].as_str().unwrap_or("?");
            let sig = m["sig"].as_str().unwrap_or(name);
            out.push_str(&format!(
                "    /// `{sig}`: {}\n    pub const {}: &str = {name:?};\n",
                m["may"].as_str().unwrap_or("?"),
                crate::testgen::ident(name)
            ));
        }
        out.push_str("}\n");
    }
    out
}

/// The links open to the app's cell, oldest first.
pub(crate) fn clients(link: &Link, cell: &str, user: &str, json: bool) -> Out {
    let mut t = connect(link, cell, user).map_err(|e| e.to_string())?;
    let list = aio::rt().block_on(t.clients()).map_err(|e| e.to_string())?;
    if json {
        return Ok(Value::Array(list).to_string());
    }
    let rows = list.iter().map(|c| {
        let since = aio::log::stamp(c["since"].as_i64().unwrap_or(0));
        let (user, wire, from) = (c["user"].as_str(), c["wire"].as_str(), c["from"].as_str());
        let rtt = c["rtt"].as_u64().map_or(String::new(), |ms| format!("  round trip {ms} ms"));
        format!(
            "{:>4}  {:<12} {:<5} {:<22} since {since}{rtt}",
            c["id"],
            user.unwrap_or("?"),
            wire.unwrap_or("?"),
            from.unwrap_or("?")
        )
    });
    // This look is one of them.
    Ok(std::iter::once(format!("{} links to {cell} (this one included)", list.len()))
        .chain(rows)
        .collect::<Vec<_>>()
        .join("\n"))
}

/// `ax timetravel`'s words: the step (none: where) and its event (pure).
pub(crate) fn travel_args<'a>(step: Option<&'a str>, seq: Option<&str>) -> Result<(&'a str, Option<u64>), String> {
    let step = step.unwrap_or("where");
    match (step, seq) {
        ("goto", Some(n)) => Ok((step, Some(n.parse().map_err(|_| format!("goto needs an event number, not {n}"))?))),
        ("goto", None) => Err("goto needs an event number: ax timetravel <app> <cell> goto 12".into()),
        (_, Some(n)) => Err(format!("{step} takes no event number ({n})")),
        ("undo" | "redo" | "pause" | "resume" | "where", None) => Ok((step, None)),
        _ => Err(format!("unknown step {step}: undo, redo, goto <seq>, pause or resume")),
    }
}

/// Where a cell is in its history, as `ax timetravel` says it (pure).
pub(crate) fn traveled(cell: &str, r: &Value) -> String {
    let paused = if r["paused"] == true { ", paused (resume: take calls again)" } else { "" };
    let head = format!("{cell} at event {} of {}{paused}; kept from event {}", r["at"], r["head"], r["floor"]);
    let steps = r["steps"].as_array().into_iter().flatten().map(|s| {
        let ms = s["ms"].as_f64().map_or(String::new(), |ms| format!("  {ms:.2} ms"));
        let here = if s["seq"] == r["at"] { "▶" } else { " " };
        format!(
            "{here}{:>6}  {}  {}{ms}",
            s["seq"].as_u64().unwrap_or(0),
            aio::log::stamp(s["at"].as_i64().unwrap_or(0)),
            s["method"].as_str().unwrap_or("?")
        )
    });
    std::iter::once(head).chain(steps).collect::<Vec<_>>().join("\n")
}

/// Step the running cell through its history, then say where it is.
pub(crate) fn travel(link: &Link, cell: &str, (step, to): (&str, Option<u64>), user: &str, json: bool) -> Out {
    let mut t = connect(link, cell, user).map_err(|e| e.to_string())?;
    let r = aio::rt().block_on(t.travel(step, to)).map_err(|e| e.why)?;
    Ok(if json { r.to_string() } else { traveled(cell, &r) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aio::prelude::*;
    use aio::serde_json::json;

    #[test]
    fn timetravel_reads_its_step_and_says_where_the_cell_is() {
        assert_eq!(travel_args(None, None), Ok(("where", None)));
        assert_eq!(travel_args(Some("undo"), None), Ok(("undo", None)));
        assert_eq!(travel_args(Some("goto"), Some("12")), Ok(("goto", Some(12))));
        assert!(travel_args(Some("goto"), None).unwrap_err().contains("needs an event number"));
        assert!(travel_args(Some("goto"), Some("x")).unwrap_err().contains("not x"));
        assert!(travel_args(Some("undo"), Some("3")).unwrap_err().contains("takes no event number"));
        assert!(travel_args(Some("back"), None).unwrap_err().contains("unknown step back"));
        let r = json!({ "at": 2, "head": 3, "floor": 0, "paused": true, "steps": [
            { "seq": 2, "method": "add", "at": 0, "ms": 0.5 }, { "seq": 3, "method": "add", "at": 0 } ] });
        let lines: Vec<String> = traveled("board", &r).lines().map(String::from).collect();
        assert_eq!(lines[0], "board at event 2 of 3, paused (resume: take calls again); kept from event 0");
        assert!(lines[1].starts_with("▶     2  ") && lines[1].ends_with("add  0.50 ms"), "{}", lines[1]);
        assert!(lines[2].starts_with("      3  ") && lines[2].ends_with("add"), "{}", lines[2]);
    }

    #[cell(shared)]
    pub struct Conn {
        pub host: String,
        pub hops: Vec<u32>,
    }

    #[methods]
    impl Conn {
        pub fn set_host(&mut self, host: String) -> Result<String> {
            ensure!(!host.is_empty(), "no host");
            self.host = host;
            Ok(self.host.clone())
        }

        pub fn hop(&mut self, n: u32) -> Result {
            self.hops.push(n);
            Ok(())
        }

        pub fn reset(&mut self) -> Result {
            self.hops.clear();
            Ok(())
        }
    }

    #[test]
    fn a_path_picks_keys_indexes_and_every_item() {
        let v = json!({ "notes": [{ "text": "a", "tags": ["x"] }, { "text": "b", "tags": [] }], "by": { "ana": 1 } });
        assert_eq!(pick(&v, "notes.1.text"), Ok(json!("b")));
        assert_eq!(pick(&v, "notes.*.text"), Ok(json!(["a", "b"])));
        assert_eq!(pick(&v, "notes.*.tags.*"), Ok(json!([["x"], []])));
        assert_eq!(pick(&v, "by.*"), Ok(json!([1])));
        assert_eq!(pick(&v, ""), Ok(v.clone()));
        assert_eq!(pick(&v, "nope").unwrap_err(), "no `nope` in the state: it has by, notes");
        assert_eq!(pick(&v, "notes.7").unwrap_err(), "no `7` in `notes`: a list of 2");
        assert!(pick(&v, "by.ana.*").unwrap_err().contains("`by.ana` is not a list or a map"));
    }

    #[test]
    fn a_path_filters_items_and_picks_fields() {
        let v = json!({ "fleet": [
            { "name": "a", "live": true, "status": "up", "stats": { "pnl": 3 } },
            { "name": "b", "live": false, "status": "down", "stats": { "pnl": -1 } },
            { "name": "c", "live": true, "status": "down", "tags": ["x"], "stats": { "pnl": 0 } }
        ], "owner": { "id": 7, "name": "ana", "key": "s3cret" } });
        assert_eq!(pick(&v, "fleet[?live].name"), Ok(json!(["a", "c"])));
        assert_eq!(pick(&v, "fleet[?status=\"down\"].name"), Ok(json!(["b", "c"])));
        assert_eq!(pick(&v, "fleet[?status=down].name"), Ok(json!(["b", "c"])), "not JSON: text");
        assert_eq!(pick(&v, "fleet[?status!=down].name"), Ok(json!(["a"])));
        assert_eq!(pick(&v, "fleet[?stats.pnl=0].name"), Ok(json!(["c"])));
        assert_eq!(pick(&v, "fleet[?tags=[\"x\"]].name"), Ok(json!(["c"])), "a bracket inside a filter");
        // A bracket after a filter is the kept list; filters chain.
        assert_eq!(pick(&v, "fleet[?live][1].name"), Ok(json!("c")));
        assert_eq!(pick(&v, "fleet[?live][?status=up].name"), Ok(json!(["a"])));
        assert_eq!(pick(&v, "fleet[?nope=1]").unwrap_err(), "no item of `fleet` has `nope` (a typo is never a match)");
        // Picks: some fields of a map, or of each item; nested ones by path.
        assert_eq!(pick(&v, "owner.{id,name}"), Ok(json!({ "id": 7, "name": "ana" })));
        assert_eq!(pick(&v, "fleet[*].{name,stats.pnl}").unwrap()[1], json!({ "name": "b", "stats.pnl": -1 }));
        assert!(pick(&v, "owner.{nope}").unwrap_err().contains("no `{nope}` in `owner`: it has id, key, name"));
        // Brackets and dots are one grammar; a bad one says the grammar.
        assert_eq!(pick(&v, "fleet[0].name"), pick(&v, "fleet.0.name"));
        assert!(pick(&v, "fleet[x]").unwrap_err().contains("a path: a.b"));
        assert!(pick(&v, "fleet[?live").unwrap_err().contains("never closed"));
    }

    #[test]
    fn words_are_json_when_they_are_and_text_when_not() {
        // A one-string method gets a string, never an object (aio field report).
        assert_eq!(call_args(&["192.168.1.9"], None), Ok(json!(["192.168.1.9"])));
        assert_eq!(
            call_args(&["3", "true", "{\"a\":1}", "two words"], None),
            Ok(json!([3, true, { "a": 1 }, "two words"]))
        );
        assert_eq!(call_args(&["https://x.example/?a=b"], None), Ok(json!(["https://x.example/?a=b"])));
        assert_eq!(call_args(&[], None), Ok(Value::Null));
        // aio's named payload: key=value words, all of them, are one object.
        assert_eq!(
            call_args(&["title=milk", "n=2", "done=false"], None),
            Ok(json!([{ "title": "milk", "n": 2, "done": false }]))
        );
        assert_eq!(call_args(&["title=a=b"], None), Ok(json!([{ "title": "a=b" }])));
        assert_eq!(call_args(&["title=x", "plain"], None), Ok(json!(["title=x", "plain"])), "not all pairs: words");
        assert!(call_args(&["a=1", "a=2"], None).unwrap_err().contains("a= is given twice"));
        assert_eq!(body("{\"a\":1}"), Ok(json!([{ "a": 1 }])));
        assert!(body("[1]").unwrap_err().contains("one JSON object"));
        let cells = [
            json!({ "cell": "todo", "v": 2, "methods": [{ "method": "add", "sig": "add(text: String) -> u32", "may": "yes" }] }),
        ];
        let t = peer_types("notes", &cells);
        assert!(t.contains("pub mod todo {\n    pub const ID: &str = \"todo\";\n    /// `add(text: String) -> u32`: yes\n    pub const add: &str = \"add\";\n}"), "{t}");
        // Exact: --args is the list as given.
        assert_eq!(call_args(&[], Some("[\"3\"]")), Ok(json!(["3"])));
        assert_eq!(call_args(&[], Some("[]")), Ok(Value::Null));
        assert!(call_args(&[], Some("{\"args\":[1]}")).unwrap_err().contains("takes a JSON list"));
        assert!(call_args(&["1"], Some("[1]")).unwrap_err().contains("not both"));
        // A number JSON can't hold exactly is refused, not changed.
        assert!(call_args(&["123456789012345678901234567890"], None).unwrap_err().contains("--args '[\""));
        assert_eq!(call_args(&["0.5", "-3", "1e3"], None), Ok(json!([0.5, -3, 1000.0])));
    }

    #[test]
    fn expect_compares_like_json() {
        let ok = |a: Value, op: &str, raw: Option<&str>| compare(Some(&a), op, raw).0;
        // eq / ne: structural, 1 is 1.0, key order doesn't matter.
        assert!(ok(json!(5), "eq", Some("5")) && ok(json!(5.0), "eq", Some("5")));
        assert!(ok(json!("Ready"), "eq", Some("Ready")) && ok(json!("Ready"), "eq", Some("\"Ready\"")));
        assert!(!ok(json!(5), "eq", Some("6")) && ok(json!(5), "ne", Some("6")));
        assert!(ok(json!({ "a": 1, "b": 2 }), "eq", Some(r#"{"b":2,"a":1}"#)));
        assert!(!ok(json!({ "a": 1, "b": 2 }), "ne", Some(r#"{"b":2,"a":1}"#)));
        // Ordering: numbers only, and saying so.
        assert!(ok(json!(10), "gt", Some("5")) && !ok(json!(5), "gt", Some("5")) && ok(json!(5), "gte", Some("5")));
        assert!(ok(json!(3), "lt", Some("5")) && ok(json!(5), "lte", Some("5")));
        assert!(compare(Some(&json!(5)), "gt", Some("1O")).1.contains("expected 1O is not one"));
        assert!(compare(Some(&json!("x")), "gt", Some("1")).1.contains("actual \"x\" is not one"));
        // contains: a text's characters as typed, a list's items.
        assert!(ok(json!("hello world"), "contains", Some("world")) && !ok(json!("hello"), "contains", Some("xyz")));
        assert!(ok(json!("price 1.50"), "contains", Some("1.50")));
        assert!(ok(json!([1, 2, 3]), "contains", Some("2")) && !ok(json!([1, 2, 3]), "contains", Some("9")));
        assert!(!ok(json!(42), "contains", Some("4")));
        // exists / absent are about the path, not the value.
        assert!(ok(Value::Null, "exists", None) && !ok(json!(0), "absent", None));
        assert!(compare(None, "absent", None).0 && !compare(None, "exists", None).0);
        // A missing path never passes a comparison (a typo can't pass `ne`).
        assert!(compare(None, "ne", Some("0")).1.contains("path not found"));
        // The words are checked before the app is asked.
        assert!(expect_usage("bogus", Some("1")).unwrap_err().contains("unknown op `bogus`"));
        assert!(expect_usage("eq", None).unwrap_err().contains("needs a value"));
        assert!(expect_usage("exists", Some("1")).unwrap_err().contains("takes no value"));
    }

    #[cell(persist, shared, visible = mine)]
    pub struct Diary {
        pub entries: Vec<(String, String)>,
        #[server]
        pub audit: Vec<String>,
    }

    fn mine(d: &Diary, viewer: &aio::Caller) -> Diary {
        let me = match &viewer.by {
            By::User(u) => u.as_str(),
            _ => "",
        };
        Diary { entries: d.entries.iter().filter(|(w, _)| w == me).cloned().collect(), ..d.clone() }
    }

    #[methods]
    impl Diary {
        pub fn write(&mut self, who: String, text: String) -> Result {
            self.audit.push(format!("{who} wrote"));
            self.entries.push((who, text));
            Ok(())
        }
    }

    #[test]
    fn as_a_user_the_state_is_the_projection_that_user_is_sent() {
        // aio's `am state --ui <user>`: the server's projection for that user.
        let d = tempfile::tempdir().unwrap();
        let h = Handle::<Diary>::memory();
        h.write("ana".into(), "a1".into()).unwrap();
        h.write("bo".into(), "b1".into()).unwrap();
        let addr = aio::ipc::addr(d.path(), "cell-diary");
        let l = {
            let _in = aio::rt().enter();
            aio::ipc::Listener::bind(&addr).unwrap()
        };
        aio::rt().spawn(aio::sync::serve_door(h.clone(), l, Default::default()));
        let link = Link { addr, token: String::new(), root: None };
        let all: Value = aio::serde_json::from_str(&state(&link, "diary", Some(""), "ana").unwrap().unwrap()).unwrap();
        assert_eq!(all, json!({ "entries": [["ana", "a1"]] }), "bo's entry and the #[server] field are not ana's");
        assert_eq!(state(&link, "diary", Some("entries.0.0"), "bo").unwrap(), Ok("\"bo\"".into()));
        assert!(
            state(&link, "diary", Some("audit"), "ana").unwrap().unwrap_err().contains("no `audit`"),
            "#[server] stays"
        );
        // `ax peer`: what a caller may call, and how.
        let (p, _) = crate::travel::ask(&link, "diary", "ops", json!({ "t": "peer", "user": "ana" })).unwrap();
        let text = peer(std::slice::from_ref(&p), false);
        assert!(text.starts_with("diary v1\n  diary.write(who: String, text: String) -> ()"), "{text}");
        assert!(text.contains("no: access denied: `write` is not a #[client] method"), "{text}");
        assert_eq!(peer(std::slice::from_ref(&p), true).chars().next(), Some('['));
        assert_eq!(peer(&[], false), "no cell answers on the running app's door");
        // With which fields it saves and sends (`ax peer` asks the door both).
        let (f, _) = crate::travel::ask(&link, "diary", "ops", json!({ "t": "fields" })).unwrap();
        let text = peer(&[json!({ "cell": "diary", "v": 1, "fields": f, "methods": [] })], false);
        assert_eq!(text, "diary v1\n  fields: audit never sent", "{text}");
        let mem = json!({ "cell": "x", "v": 1, "fields": { "persist": false }, "methods": [] });
        assert!(peer(&[mem], false).ends_with("fields: kept in memory, never saved"));
    }

    #[test]
    fn args_too_big_for_the_command_line_come_from_a_file() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("args.json");
        let big = "x".repeat(300_000);
        std::fs::write(&f, json!([big]).to_string()).unwrap();
        assert_eq!(call_args(&[], Some(&format!("@{}", f.display()))), Ok(json!([big])));
        assert!(call_args(&[], Some("@/no/such/file")).unwrap_err().starts_with("--args @/no/such/file"));
    }

    #[test]
    fn the_verbs_read_call_and_list_a_running_app() {
        let d = tempfile::tempdir().unwrap();
        let h = Handle::<Conn>::memory();
        let addr = aio::ipc::addr(d.path(), "cell-conn");
        let l = {
            let _in = aio::rt().enter();
            aio::ipc::Listener::bind(&addr).unwrap()
        };
        aio::rt().spawn(aio::sync::serve_door(h.clone(), l, Default::default()));
        let link = Link { addr, token: String::new(), root: None };
        assert_eq!(
            call(&link, "conn", "set_host", call_args(&["192.168.1.9"], None).unwrap(), "ops"),
            Ok("\"192.168.1.9\"".into())
        );
        assert_eq!(h.state().host, "192.168.1.9");
        call(&link, "conn", "hop", call_args(&["7"], None).unwrap(), "ops").unwrap();
        assert_eq!(call(&link, "conn", "set_host", json!([""]), "ops"), Err("refused: no host".into()));
        // Words too many or too few are refused, nothing applied (aio ran the
        // call and only warned).
        let wrong = [call(&link, "conn", "hop", json!([7, 8]), "ops"), call(&link, "conn", "hop", Value::Null, "ops")];
        assert!(wrong.iter().all(|r| r.as_ref().is_err_and(|e| e.starts_with("refused: "))), "{wrong:?}");
        assert_eq!(h.state().hops, [7]);
        // The door is the app's own (only this OS user reaches it): a method
        // no client may call is called, as aio's --as-server did.
        // A method without parameters takes no words.
        assert_eq!(call(&link, "conn", "reset", call_args(&[], None).unwrap(), "ops"), Ok("null".into()));
        call(&link, "conn", "hop", json!([9]), "ops").unwrap();
        let st = state(&link, "conn", None, "ops").unwrap().unwrap();
        assert!(st.starts_with("conn v1 at event 4 (live)\n"), "{st}");
        assert_eq!(state(&link, "conn", Some("hops.0"), "ops").unwrap(), Ok("9".into()));
        assert!(state(&link, "conn", Some("port"), "ops").unwrap().unwrap_err().contains("no `port`"));
        let now = std::time::Duration::ZERO;
        assert_eq!(expect(&link, "conn", ("hops.0", "eq", Some("9")), now, "ops"), Ok("PASS  hops.0 eq 9".into()));
        assert!(
            expect(&link, "conn", ("hops", "contains", Some("7")), now, "ops").unwrap_err().starts_with("FAIL  hops")
        );
        // --wait: it passes once a change makes it true.
        let h2 = h.clone();
        let later = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            h2.call("hop", json!([7])).unwrap();
        });
        let waited = expect(&link, "conn", ("hops", "contains", Some("7")), std::time::Duration::from_secs(10), "ops");
        later.join().unwrap();
        assert_eq!(waited, Ok("PASS  hops contains 7".into()));
        call(&link, "conn", "reset", Value::Null, "ops").unwrap();
        call(&link, "conn", "hop", json!([9]), "ops").unwrap();
        // The links of the calls above close on their own time: wait for them.
        let mut ls = clients(&link, "conn", "ops", false).unwrap();
        for _ in 0..100 {
            if ls.starts_with("1 links to conn") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            ls = clients(&link, "conn", "ops", false).unwrap();
        }
        assert!(ls.starts_with("1 links to conn"), "{ls}");
        assert!(ls.contains("ops") && ls.contains("tool") && ls.contains("door"), "{ls}");
        let rows: Value = serde_json::from_str(&clients(&link, "conn", "ops", true).unwrap()).unwrap();
        assert_eq!((rows[0]["user"].as_str(), rows[0]["wire"].as_str()), (Some("ops"), Some("tool")));
        // No app on the door: an error to fall back on, nothing sent.
        let gone = Link { addr: aio::ipc::addr(d.path(), "cell-none"), ..link };
        assert!(state(&gone, "conn", None, "ops").is_err());
        assert!(call(&gone, "conn", "reset", Value::Null, "ops").is_err());
    }
}

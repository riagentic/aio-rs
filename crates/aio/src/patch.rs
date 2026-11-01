//! Patches: the change between two JSON values, as a list of set/delete ops
//! by path. Objects are compared key by key, same-length arrays item by item;
//! an array that grew or shrank is sent as the items put in (`insert`) or
//! taken out (`del` of an index), as aio narrows Immer's patches: a push, a
//! spread or a filter costs its items, not the array; when that would carry
//! as much as the array itself, the array goes whole. A long string that grew
//! is sent as its new tail (`append`, as aio: streamed text costs its growth,
//! not its length); anything else is replaced whole. `apply(old, diff(old, new)) == new`. A patch carries the
//! [`digest`] of the value it makes, so a copy that patched its way somewhere
//! else finds out at once (aio's state digest).
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One step of a path: an object key or an array index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Step {
    Key(String),
    Index(usize),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Op {
    Set {
        path: Vec<Step>,
        value: Value,
    },
    /// The key at `path` is gone; at an array index: that item, later ones
    /// move down.
    Del {
        path: Vec<Step>,
    },
    /// `value` put in at array index `path` (the array's length: at its
    /// end), later items move up.
    Insert {
        path: Vec<Step>,
        value: Value,
    },
    /// The string at `path` grew by `value`.
    Append {
        path: Vec<Step>,
        value: String,
    },
}

/// A string shorter than this is replaced, not appended to (aio's
/// `APPEND_MIN_LENGTH`: below it the op costs more than it saves).
pub const APPEND_MIN: usize = 256;

pub fn diff(old: &Value, new: &Value) -> Vec<Op> {
    let mut ops = Vec::new();
    walk(old, new, &mut Vec::new(), &mut ops);
    ops
}

fn walk(old: &Value, new: &Value, path: &mut Vec<Step>, ops: &mut Vec<Op>) {
    if old == new {
        return;
    }
    match (old, new) {
        (Value::Object(a), Value::Object(b)) => {
            for k in a.keys().filter(|k| !b.contains_key(*k)) {
                path.push(Step::Key(k.clone()));
                ops.push(Op::Del { path: path.clone() });
                path.pop();
            }
            for (k, v) in b {
                path.push(Step::Key(k.clone()));
                match a.get(k) {
                    Some(old) => walk(old, v, path, ops),
                    None => ops.push(Op::Set { path: path.clone(), value: v.clone() }),
                }
                path.pop();
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => items(a, b, path, ops),
        (Value::Array(a), Value::Array(b)) => resized(a, b, path, ops),
        (Value::String(a), Value::String(b)) if a.len() >= APPEND_MIN && b.starts_with(a.as_str()) => {
            ops.push(Op::Append { path: path.clone(), value: b[a.len()..].to_string() })
        }
        _ => ops.push(Op::Set { path: path.clone(), value: new.clone() }),
    }
}

/// Same-length arrays, item by item.
fn items(a: &[Value], b: &[Value], path: &mut Vec<Step>, ops: &mut Vec<Op>) {
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        path.push(Step::Index(i));
        walk(x, y, path, ops);
        path.pop();
    }
}

/// The indexes of `long` that aren't in `short`, when `short` is `long` with
/// only some items taken out (in order: the first match is taken).
fn left_out(long: &[Value], short: &[Value]) -> Option<Vec<usize>> {
    let mut j = 0;
    let out: Vec<usize> = (0..long.len())
        .filter(|&i| match short.get(j) {
            Some(x) if *x == long[i] => {
                j += 1;
                false
            }
            _ => true,
        })
        .collect();
    (j == short.len()).then_some(out)
}

/// An array that grew or shrank: the cheapest on the wire of its items put in
/// or taken out (a push, a spread, a filter), its common part item by item
/// with the rest put in or cut off at the end, or the array whole.
fn resized(a: &[Value], b: &[Value], path: &mut Vec<Step>, ops: &mut Vec<Op>) {
    let at = |path: &Vec<Step>, i: usize| [&path[..], &[Step::Index(i)]].concat();
    let mut best = None::<(usize, Vec<Op>)>;
    let mut weigh = |cand: Vec<Op>| {
        let size = serde_json::to_string(&cand).map_or(usize::MAX, |s| s.len());
        if best.as_ref().is_none_or(|(n, _)| size < *n) {
            best = Some((size, cand));
        }
    };
    // Only taken out (from the back, so each index is still the old one's),
    // or only put in (from the front, at each item's new index).
    if let Some(out) = left_out(a, b) {
        weigh(out.into_iter().rev().map(|i| Op::Del { path: at(path, i) }).collect());
    } else if let Some(put) = left_out(b, a) {
        weigh(put.into_iter().map(|i| Op::Insert { path: at(path, i), value: b[i].clone() }).collect());
    }
    let n = a.len().min(b.len());
    let mut common = Vec::new();
    items(&a[..n], &b[..n], path, &mut common);
    common.extend((n..b.len()).map(|i| Op::Insert { path: at(path, i), value: b[i].clone() }));
    common.extend((n..a.len()).rev().map(|i| Op::Del { path: at(path, i) }));
    weigh(common);
    let whole = Op::Set { path: path.clone(), value: Value::Array(b.to_vec()) };
    match best {
        Some((size, cand)) if size < serde_json::to_string(&[&whole]).map_or(0, |s| s.len()) => ops.extend(cand),
        _ => ops.push(whole),
    }
}

/// `ops` against the `old` they were made from, for a person (pure):
/// `items.3.done false→true, title "a"→"b"`. Each value at most 80
/// characters, strings quoted (`"1"→1` is a change).
pub fn said(old: &Value, ops: &[Op]) -> String {
    let at = |path: &[Step]| {
        path.iter().try_fold(old, |v, s| match s {
            Step::Key(k) => v.get(k),
            Step::Index(i) => v.get(i),
        })
    };
    let name = |path: &[Step]| {
        let parts: Vec<String> = path
            .iter()
            .map(|s| match s {
                Step::Key(k) => k.clone(),
                Step::Index(i) => i.to_string(),
            })
            .collect();
        if parts.is_empty() { "(all)".into() } else { parts.join(".") }
    };
    let short = |v: Option<&Value>| {
        let s = v.map_or("none".into(), Value::to_string);
        match s.char_indices().nth(80) {
            Some((i, _)) => format!("{}…", &s[..i]),
            None => s,
        }
    };
    let one = |op: &Op| match op {
        Op::Set { path, value } => format!("{} {}→{}", name(path), short(at(path)), short(Some(value))),
        Op::Del { path } => format!("{} {}→none", name(path), short(at(path))),
        Op::Insert { path, value } => format!("{} +{}", name(path), short(Some(value))),
        Op::Append { path, value } => format!("{} +{} chars", name(path), value.chars().count()),
    };
    ops.iter().map(one).collect::<Vec<_>>().join(", ")
}

/// Apply `ops` to `v`. A path that doesn't fit `v` is an error: the copy and
/// the patch disagree, so the caller takes the full state.
pub fn apply(v: &mut Value, ops: &[Op]) -> Result<(), String> {
    for op in ops {
        let (path, value) = match op {
            Op::Set { path, value } => (path, Some(value.clone())),
            Op::Del { path } => (path, None),
            Op::Insert { path, value } => {
                let Some((Step::Index(i), parents)) = path.split_last() else {
                    return Err("insert at a path that ends in no index".into());
                };
                match parents.iter().try_fold(&mut *v, |at, s| match (s, at) {
                    (Step::Key(k), Value::Object(m)) => m.get_mut(k),
                    (Step::Index(i), Value::Array(a)) => a.get_mut(*i),
                    _ => None,
                }) {
                    Some(Value::Array(a)) if *i <= a.len() => a.insert(*i, value.clone()),
                    _ => return Err(format!("no array to insert into at index {i}")),
                }
                continue;
            }
            Op::Append { path, value } => {
                let old = path.iter().try_fold(&*v, |at, s| match s {
                    Step::Key(k) => at.get(k),
                    Step::Index(i) => at.get(i),
                });
                let Some(Value::String(old)) = old else {
                    return Err("append to a missing or non-string value".into());
                };
                (path, Some(Value::String(format!("{old}{value}"))))
            }
        };
        let Some((last, parents)) = path.split_last() else {
            match value {
                Some(x) => *v = x,
                None => return Err("can't delete the root".into()),
            }
            continue;
        };
        let mut at = &mut *v;
        for s in parents {
            at = match (s, at) {
                (Step::Key(k), Value::Object(m)) => m.get_mut(k).ok_or_else(|| format!("no key {k}"))?,
                (Step::Index(i), Value::Array(a)) => a.get_mut(*i).ok_or_else(|| format!("no index {i}"))?,
                (s, _) => return Err(format!("path step {s:?} doesn't fit")),
            };
        }
        match (last, at, value) {
            (Step::Key(k), Value::Object(m), Some(x)) => _ = m.insert(k.clone(), x),
            (Step::Key(k), Value::Object(m), None) => _ = m.remove(k).ok_or_else(|| format!("no key {k}"))?,
            (Step::Index(i), Value::Array(a), Some(x)) => {
                *a.get_mut(*i).ok_or_else(|| format!("no index {i}"))? = x.clone()
            }
            (Step::Index(i), Value::Array(a), None) if *i < a.len() => _ = a.remove(*i),
            (s, _, _) => return Err(format!("path step {s:?} doesn't fit")),
        }
    }
    Ok(())
}

/// The digest of `v`: SHA-256 of its JSON with object keys sorted (the same
/// value digests the same, whatever its key order), the first 16 bytes in
/// hex. Part of the wire: both sides compare it.
pub fn digest(v: &Value) -> String {
    fn feed(v: &Value, h: &mut crate::sha256::Sha256) {
        let list = |h: &mut crate::sha256::Sha256, i: usize| h.update(if i == 0 { &b""[..] } else { b"," });
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                h.update(b"{");
                for (i, k) in keys.into_iter().enumerate() {
                    list(h, i);
                    h.update(Value::String(k.clone()).to_string().as_bytes());
                    h.update(b":");
                    feed(&m[k], h);
                }
                h.update(b"}");
            }
            Value::Array(a) => {
                h.update(b"[");
                for (i, x) in a.iter().enumerate() {
                    list(h, i);
                    feed(x, h);
                }
                h.update(b"]");
            }
            x => h.update(x.to_string().as_bytes()),
        }
    }
    let mut h = crate::sha256::Sha256::new();
    feed(v, &mut h);
    h.finalize()[..16].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_change_is_said_key_by_key_with_strings_quoted() {
        let old = json!({ "items": [{ "done": false }], "title": "a", "n": "1", "gone": 2 });
        let new = json!({ "items": [{ "done": true }], "title": "b", "n": 1, "added": null });
        let one = said(&old, &diff(&old, &new));
        assert_eq!(one, "gone 2→none, added none→null, items.0.done false→true, n \"1\"→1, title \"a\"→\"b\"");
        let long = json!("x".repeat(300));
        assert_eq!(said(&json!(1), &diff(&json!(1), &long)), format!("(all) 1→\"{}…", "x".repeat(79)));
        let grown = json!("y".repeat(APPEND_MIN + 5));
        assert_eq!(
            said(&json!("y".repeat(APPEND_MIN)), &diff(&json!("y".repeat(APPEND_MIN)), &grown)),
            "(all) +5 chars"
        );
    }

    fn round(a: Value, b: Value) -> Vec<Op> {
        let ops = diff(&a, &b);
        let mut c = a.clone();
        apply(&mut c, &ops).unwrap();
        assert_eq!(c, b);
        ops
    }

    #[test]
    fn small_changes_make_small_patches() {
        let ops = round(
            json!({ "a": 1, "rows": { "x": { "t": "a", "done": false } } }),
            json!({ "a": 1, "rows": { "x": { "t": "a", "done": true } } }),
        );
        assert_eq!(
            ops,
            vec![Op::Set {
                path: vec![Step::Key("rows".into()), Step::Key("x".into()), Step::Key("done".into())],
                value: json!(true)
            }]
        );
        assert!(round(json!({ "a": [1, 2] }), json!({ "a": [1, 2] })).is_empty());
    }

    #[test]
    fn adds_deletes_nulls_and_resized_arrays() {
        round(json!({ "a": 1, "b": 2 }), json!({ "b": null, "c": [1] }));
        round(json!({ "a": [1, 2, 3] }), json!({ "a": [1] }));
        round(json!({ "a": [{ "x": 1 }, { "x": 2 }] }), json!({ "a": [{ "x": 1 }, { "x": 3, "y": 0 }] }));
        round(json!(1), json!("s"));
    }

    #[test]
    fn a_resized_array_sends_the_items_put_in_or_taken_out() {
        let rows: Vec<Value> =
            (0..160).map(|i| json!({ "id": i, "name": format!("row {i}"), "pad": "x".repeat(40) })).collect();
        let at = |i: usize| vec![Step::Key("c".into()), Step::Index(i)];
        // A push (or a spread with new items at the end): one insert per item.
        let mut pushed = rows.clone();
        pushed.extend([json!(1), json!(2)]);
        assert_eq!(
            round(json!({ "c": rows }), json!({ "c": pushed })),
            vec![Op::Insert { path: at(160), value: json!(1) }, Op::Insert { path: at(161), value: json!(2) }]
        );
        // A filter: one del per item, from the back.
        let kept: Vec<Value> = rows.iter().filter(|r| ![3, 90].contains(&r["id"].as_i64().unwrap())).cloned().collect();
        assert_eq!(
            round(json!({ "c": rows }), json!({ "c": kept })),
            vec![Op::Del { path: at(90) }, Op::Del { path: at(3) }]
        );
        // Put in at the front and in the middle, at their new indexes.
        let mut mid = rows.clone();
        mid.insert(0, json!("a"));
        mid.insert(50, json!("b"));
        assert_eq!(
            round(json!({ "c": rows }), json!({ "c": mid })),
            vec![Op::Insert { path: at(0), value: json!("a") }, Op::Insert { path: at(50), value: json!("b") }]
        );
        // An edit and a push: item by item, then the new one.
        let mut both = rows.clone();
        both[7]["name"] = json!("seven");
        both.push(json!(0));
        let ops = round(json!({ "c": rows }), json!({ "c": both }));
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[1], Op::Insert { path, .. } if *path == at(160)));
        // Cut at the end: dels from the back; a small array goes whole.
        assert_eq!(
            round(json!({ "c": rows }), json!({ "c": rows[..158] })),
            vec![Op::Del { path: at(159) }, Op::Del { path: at(158) }]
        );
        assert!(matches!(round(json!([1, 2, 3, 4]), json!([1, 2]))[..], [Op::Set { .. }]));
        // A reorder or a rebuild that costs as much as the array: whole.
        assert!(matches!(round(json!([1, 2, 3]), json!([3, 2, 1, 0]))[..], [Op::Set { .. }]));
        assert_eq!(round(json!([1]), json!([])), vec![Op::Del { path: vec![Step::Index(0)] }]);
        // Duplicates match the first one: still right.
        round(json!([1, 1, 2, 1]), json!([1, 2, 1]));
        round(json!([1, 2]), json!([1, 1, 2, 2, 1]));
        round(json!([[1, 2], [3]]), json!([[1, 2, 3]]));
        assert_eq!(
            said(&json!([1, 2]), &diff(&json!([1, 2]), &json!([1, 2, 3, 4, 5, 6, 7, 8]))).split(", ").next(),
            Some("2 +3")
        );
    }

    #[test]
    fn inserts_and_index_dels_fit_only_an_array() {
        let ins = |path: Vec<Step>| Op::Insert { path, value: json!(0) };
        assert!(apply(&mut json!([1]), &[ins(vec![Step::Index(2)])]).is_err(), "past the end");
        assert!(apply(&mut json!({ "a": 1 }), &[ins(vec![Step::Key("a".into())])]).is_err(), "no index");
        assert!(apply(&mut json!({ "a": 1 }), &[ins(vec![Step::Key("a".into()), Step::Index(0)])]).is_err());
        assert!(apply(&mut json!([1]), &[Op::Del { path: vec![Step::Index(1)] }]).is_err());
        let mut v = json!({ "a": [1, 2] });
        apply(
            &mut v,
            &[
                ins(vec![Step::Key("a".into()), Step::Index(2)]),
                Op::Del { path: vec![Step::Key("a".into()), Step::Index(0)] },
            ],
        )
        .unwrap();
        assert_eq!(v, json!({ "a": [2, 0] }));
        assert_eq!(
            serde_json::to_value(ins(vec![Step::Index(1)])).unwrap(),
            json!({ "op": "insert", "path": [1], "value": 0 })
        );
    }

    #[test]
    fn a_patch_that_doesnt_fit_is_an_error() {
        let ops = diff(&json!({ "a": { "b": 1 } }), &json!({ "a": { "b": 2 } }));
        assert!(apply(&mut json!({ "z": 0 }), &ops).is_err());
        assert!(apply(&mut json!({}), &[Op::Del { path: vec![Step::Key("q".into())] }]).is_err());
    }

    #[test]
    fn a_long_string_that_grew_sends_its_tail() {
        let long = "t".repeat(APPEND_MIN);
        let ops = round(json!({ "reply": long }), json!({ "reply": format!("{long} more") }));
        assert_eq!(ops, vec![Op::Append { path: vec![Step::Key("reply".into())], value: " more".into() }]);
        assert_eq!(
            serde_json::to_value(&ops[0]).unwrap(),
            json!({ "op": "append", "path": ["reply"], "value": " more" })
        );
        // Short, or not a growth: replaced.
        assert!(matches!(round(json!("ab"), json!("abc"))[..], [Op::Set { .. }]));
        assert!(matches!(round(json!(long.clone()), json!(format!("x{long}")))[..], [Op::Set { .. }]));
        assert!(apply(&mut json!({ "reply": 1 }), &ops).is_err());
    }

    #[test]
    fn a_digest_is_of_the_value_not_its_key_order() {
        let a: Value = serde_json::from_str(r#"{"b":[1,{"y":2,"x":"s"}],"a":null}"#).unwrap();
        let b = json!({ "a": null, "b": [1, { "x": "s", "y": 2 }] });
        assert_eq!((digest(&a), digest(&a).len()), (digest(&b), 32));
        for other in [json!({ "a": null, "b": [1, { "x": "s", "y": 3 }] }), json!({ "a": null }), json!([null])] {
            assert_ne!(digest(&a), digest(&other));
        }
    }

    #[test]
    fn ops_have_a_plain_wire_form() {
        let op = Op::Set { path: vec![Step::Key("rows".into()), Step::Index(2)], value: json!(1) };
        assert_eq!(serde_json::to_value(&op).unwrap(), json!({ "op": "set", "path": ["rows", 2], "value": 1 }));
    }
}

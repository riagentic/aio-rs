//! What of a cell's state is saved (aio's `persist: { include, exclude }`):
//! dot paths into the state (`feeds.busy`), a `*` segment of an exclude path
//! naming any key (`feeds.*.busy`). A list is walked item by item (an index
//! never takes a segment, but a `*` before more segments also stands for the
//! index). A `"*"` key in the data is read literally, as aio reads it. A
//! top-level key that is itself named `a.b` is excluded by `"a.b"` too.
//! Pure functions on JSON, the same in a page and in the store.
use serde_json::{Map, Value};

/// A cell's saved part: only `include` (when not empty), without `exclude`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub include: &'static [&'static str],
    pub exclude: &'static [&'static str],
}

impl Filter {
    /// Saves everything.
    pub fn is_all(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }
}

fn segs(path: &str) -> Vec<&str> {
    path.split('.').collect()
}

/// `v` without what `f` doesn't save.
pub fn strip(v: &mut Value, f: Filter) {
    if !f.include.is_empty() {
        let paths: Vec<Vec<&str>> = f.include.iter().map(|p| segs(p)).collect();
        let paths: Vec<&[&str]> = paths.iter().map(Vec::as_slice).collect();
        *v = keep(v, &paths).unwrap_or_else(|| Value::Object(Map::new()));
    }
    for p in f.exclude {
        if let Some(m) = v.as_object_mut() {
            m.remove(*p);
        }
        if p.contains('.') {
            del(v, &segs(p));
        }
    }
}

/// `saved` (what `f` saves) with what `f` doesn't save taken from `from`: the
/// state a saved copy and the state it was made of agree on.
pub fn graft(saved: &Value, from: &Value, f: Filter) -> Value {
    let mut out = match f.include.is_empty() {
        true => saved.clone(),
        false => {
            let paths: Vec<Vec<&str>> = f.include.iter().map(|p| segs(p)).collect();
            let paths: Vec<&[&str]> = paths.iter().map(Vec::as_slice).collect();
            overlay(from, Some(saved), &paths).unwrap_or(Value::Null)
        }
    };
    for p in f.exclude {
        if let (Some(m), Some(x)) = (out.as_object_mut(), from.get(*p)) {
            m.insert((*p).into(), x.clone());
        }
        if p.contains('.') {
            put_back(&mut out, from, &segs(p));
        }
    }
    out
}

/// `out` with what `stored` holds and `out` lacks put back, nested objects
/// too (aio's `reattachUndeclared`): an older build's write keeps a newer
/// build's fields it doesn't know.
pub fn reattach(out: &mut Value, stored: &Value) {
    let (Some(o), Some(s)) = (out.as_object_mut(), stored.as_object()) else { return };
    for (k, v) in s {
        match o.get_mut(k) {
            None => _ = o.insert(k.clone(), v.clone()),
            Some(x) if x.is_object() && v.is_object() => reattach(x, v),
            Some(_) => {}
        }
    }
}

/// Is `head` at an object read literally (not as any key)?
fn literal(head: &str, m: &Map<String, Value>) -> bool {
    head != "*" || m.contains_key("*")
}

fn del(v: &mut Value, p: &[&str]) {
    let Some((head, rest)) = p.split_first() else { return };
    match v {
        Value::Array(a) => {
            for x in a {
                del(x, p);
                if *head == "*" && !rest.is_empty() {
                    del(x, rest);
                }
            }
        }
        Value::Object(m) => match (rest.is_empty(), literal(head, m)) {
            (true, true) => _ = m.remove(*head),
            (true, false) => m.clear(),
            (false, true) => {
                if let Some(x) = m.get_mut(*head) {
                    del(x, rest);
                }
            }
            (false, false) => m.values_mut().for_each(|x| del(x, rest)),
        },
        _ => {}
    }
}

fn put_back(v: &mut Value, from: &Value, p: &[&str]) {
    let Some((head, rest)) = p.split_first() else { return };
    match (v, from) {
        (Value::Array(a), Value::Array(f)) => {
            for (x, y) in a.iter_mut().zip(f) {
                put_back(x, y, p);
                if *head == "*" && !rest.is_empty() {
                    put_back(x, y, rest);
                }
            }
        }
        (Value::Object(m), Value::Object(f)) => match (rest.is_empty(), literal(head, f) || m.contains_key("*")) {
            (true, true) => {
                if let Some(y) = f.get(*head) {
                    m.insert((*head).into(), y.clone());
                }
            }
            (true, false) => f.iter().for_each(|(k, y)| _ = m.insert(k.clone(), y.clone())),
            (false, true) => {
                if let (Some(x), Some(y)) = (m.get_mut(*head), f.get(*head)) {
                    put_back(x, y, rest);
                }
            }
            (false, false) => {
                for (k, x) in m.iter_mut() {
                    if let Some(y) = f.get(k) {
                        put_back(x, y, rest);
                    }
                }
            }
        },
        _ => {}
    }
}

/// What `paths` pick of `v` (`None`: nothing).
fn keep(v: &Value, paths: &[&[&str]]) -> Option<Value> {
    if paths.iter().any(|p| p.is_empty()) {
        return Some(v.clone());
    }
    match v {
        Value::Object(m) => Some(Value::Object(
            m.iter()
                .filter_map(|(k, x)| {
                    let sub: Vec<&[&str]> = paths.iter().filter(|p| p[0] == k).map(|p| &p[1..]).collect();
                    (!sub.is_empty()).then(|| keep(x, &sub)).flatten().map(|x| (k.clone(), x))
                })
                .collect(),
        )),
        Value::Array(a) => {
            Some(Value::Array(a.iter().map(|x| keep(x, paths).unwrap_or_else(|| Value::Object(Map::new()))).collect()))
        }
        _ => None,
    }
}

/// `base` with what `paths` pick taken from `kept` instead (`None`: absent).
fn overlay(base: &Value, kept: Option<&Value>, paths: &[&[&str]]) -> Option<Value> {
    if paths.iter().any(|p| p.is_empty()) {
        return kept.cloned();
    }
    match (base, kept) {
        (Value::Object(b), k) => {
            let k = k.and_then(Value::as_object);
            let mut out = Map::new();
            for key in b.keys().chain(k.into_iter().flat_map(Map::keys)) {
                if out.contains_key(key) {
                    continue;
                }
                let sub: Vec<&[&str]> = paths.iter().filter(|p| p[0] == key).map(|p| &p[1..]).collect();
                let got = k.and_then(|k| k.get(key));
                let v = match (sub.is_empty(), b.get(key)) {
                    (true, b) => b.cloned(),
                    (false, Some(bv)) => overlay(bv, got, &sub),
                    (false, None) => got.cloned(),
                };
                if let Some(v) = v {
                    out.insert(key.clone(), v);
                }
            }
            Some(Value::Object(out))
        }
        (Value::Array(b), Some(Value::Array(k))) => Some(Value::Array(
            k.iter()
                .enumerate()
                .map(|(i, kv)| match b.get(i) {
                    Some(bv) => overlay(bv, Some(kv), paths).unwrap_or(Value::Null),
                    None => kv.clone(),
                })
                .collect(),
        )),
        _ => Some(base.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const EX: Filter = Filter { include: &[], exclude: &["feeds.*.busy", "cache", "a.b", "panel.tabs.*"] };

    #[test]
    fn exclude_paths_reach_into_maps_and_lists_and_come_back_from_the_state() {
        let st = json!({
            "feeds": { "x": { "url": "u", "busy": true }, "y": { "url": "v", "busy": false } },
            "cache": [1, 2],
            "a.b": "literal key", "a": { "b": 1, "c": 2 },
            "panel": { "tabs": { "t1": 1 }, "open": true },
            "n": 3,
        });
        let mut saved = st.clone();
        strip(&mut saved, EX);
        assert_eq!(
            saved,
            json!({ "feeds": { "x": { "url": "u" }, "y": { "url": "v" } }, "a": { "c": 2 }, "panel": { "tabs": {}, "open": true }, "n": 3 })
        );
        assert_eq!(graft(&saved, &st, EX), st);
        // A list of feeds: each item's `busy` (the `*` stands for the index too).
        let list = json!({ "feeds": [{ "url": "u", "busy": true }, { "url": "v" }] });
        let mut saved = list.clone();
        strip(&mut saved, EX);
        assert_eq!(saved, json!({ "feeds": [{ "url": "u" }, { "url": "v" }] }));
        assert_eq!(graft(&saved, &list, EX), list);
        // A literal "*" key is that key only.
        let star = json!({ "feeds": { "*": { "busy": 1, "k": 2 }, "z": { "busy": 3 } } });
        let mut saved = star.clone();
        strip(&mut saved, EX);
        assert_eq!(saved, json!({ "feeds": { "*": { "k": 2 }, "z": { "busy": 3 } } }));
        assert_eq!(graft(&saved, &star, EX), star);
    }

    #[test]
    fn a_newer_builds_fields_are_put_back_and_never_over_this_ones() {
        let mut out = json!({ "n": 2, "inner": { "a": 1 } });
        reattach(&mut out, &json!({ "n": 1, "later": [1], "inner": { "a": 0, "b": true } }));
        assert_eq!(out, json!({ "n": 2, "later": [1], "inner": { "a": 1, "b": true } }));
    }

    #[test]
    fn include_paths_keep_only_what_they_name() {
        let f = Filter { include: &["name", "profile.email", "rows.id"], exclude: &[] };
        let st = json!({
            "name": "ana", "secret": 1,
            "profile": { "email": "e", "token": "t" },
            "rows": [{ "id": 1, "tmp": true }, { "id": 2 }],
        });
        let mut saved = st.clone();
        strip(&mut saved, f);
        assert_eq!(saved, json!({ "name": "ana", "profile": { "email": "e" }, "rows": [{ "id": 1 }, { "id": 2 }] }));
        assert_eq!(graft(&saved, &st, f), st);
        // What it saves changed: the saved copy wins there, the rest is the state's.
        let changed = json!({ "name": "bo", "profile": { "email": "f" }, "rows": [{ "id": 9 }] });
        assert_eq!(
            graft(&changed, &st, f),
            json!({ "name": "bo", "secret": 1, "profile": { "email": "f", "token": "t" }, "rows": [{ "id": 9, "tmp": true }] })
        );
        assert!(Filter::default().is_all() && !f.is_all());
    }
}

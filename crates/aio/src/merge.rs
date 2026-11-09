//! Merging concurrent edits (aio's merge strategies). A server replays each
//! method in order, so what a method expresses merges by itself: `n += 1`
//! from two offline clients counts twice (aio's `counter`), `insert`/`remove`
//! on a set keep both sides (aio's `set-add`/`set-remove`). Prose is the one
//! value a method can't merge by itself: [`text3`] merges two edits of a
//! text against the text both started from (aio's `text`), for a method like
//! `edit(id, base, new)`.
//!
//! A diff3, not a sequence CRDT: edits to different parts both survive; two
//! edits of the same part are a conflict, the later one wins and it is said
//! (`Merged::conflict`), never a silent loss or a mangled mix.
//!
//! A synced copy shows its waiting calls replayed on the server's state.
//! Where a server change meets one of them and the two disagree, the copy
//! says so ([`conflicts`], aio's `onConflict`). A field may be shown merged
//! while both changed it ([`Merge`], aio's per-field view merge): what the
//! server keeps is still what its methods make.
use serde_json::Value;

/// Sides longer than this (in tokens), or a diff table bigger than
/// [`MAX_CELLS`], aren't diffed: the later edit wins, as a conflict.
pub const MAX_TOKENS: usize = 4000;
pub const MAX_CELLS: usize = 4_000_000;

/// What [`text3`] made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merged {
    pub text: String,
    /// Both edits changed the same part: `mine`'s change was kept there.
    pub conflict: bool,
}

/// Text split the way people edit it: by line when it has lines (each with
/// its newline), else by character (a title, a name).
pub fn tokens(s: &str) -> Vec<&str> {
    if s.contains('\n') {
        s.split_inclusive('\n').collect()
    } else {
        s.char_indices().map(|(i, c)| &s[i..i + c.len_utf8()]).collect()
    }
}

/// One edit over the base: `base[from..to]` becomes `insert`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Hunk<'a> {
    from: usize,
    to: usize,
    insert: Vec<&'a str>,
}

/// The longest common subsequence of `a` and `b`, as matched index pairs;
/// ties are broken one fixed way, so every side computes the same merge.
fn lcs(a: &[&str], b: &[&str]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut t = vec![0u32; (n + 1) * (m + 1)];
    for i in 1..=n {
        for j in 1..=m {
            t[i * (m + 1) + j] = if a[i - 1] == b[j - 1] {
                t[(i - 1) * (m + 1) + j - 1] + 1
            } else {
                t[(i - 1) * (m + 1) + j].max(t[i * (m + 1) + j - 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (n, m, vec![]);
    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            out.push((i - 1, j - 1));
            (i, j) = (i - 1, j - 1);
        } else if t[(i - 1) * (m + 1) + j] >= t[i * (m + 1) + j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    out.reverse();
    out
}

/// The edits that make `next` from `base`, as disjoint hunks over `base`.
fn hunks<'a>(base: &[&'a str], next: &[&'a str]) -> Vec<Hunk<'a>> {
    let lo = base.iter().zip(next).take_while(|(a, b)| a == b).count();
    let tail = base[lo..].iter().rev().zip(next[lo..].iter().rev()).take_while(|(a, b)| a == b).count();
    let (a, b) = (&base[lo..base.len() - tail], &next[lo..next.len() - tail]);
    let mut out = vec![];
    let mut push = |from: usize, to: usize, insert: &[&'a str]| {
        if from < to || !insert.is_empty() {
            out.push(Hunk { from: lo + from, to: lo + to, insert: insert.to_vec() });
        }
    };
    let (mut ai, mut bi) = (0, 0);
    for (ma, mb) in lcs(a, b) {
        push(ai, ma, &b[bi..mb]);
        (ai, bi) = (ma + 1, mb + 1);
    }
    push(ai, a.len(), &b[bi..]);
    out
}

/// Do two hunks touch the same part of the base? An insertion at the edge
/// of a replaced range doesn't; two insertions at one point do (no order).
fn overlaps(x: &Hunk, y: &Hunk) -> bool {
    match (x.from == x.to, y.from == y.to) {
        (true, true) => x.from == y.from,
        (true, false) => x.from > y.from && x.from < y.to,
        (false, true) => y.from > x.from && y.from < x.to,
        (false, false) => x.from < y.to && y.from < x.to,
    }
}

/// Merge `mine` (the later edit) and `theirs` (what is there now), both
/// made from `base`.
pub fn text3(base: &str, mine: &str, theirs: &str) -> Merged {
    let clean = |text: &str| Merged { text: text.into(), conflict: false };
    if mine == theirs || base == theirs {
        return clean(mine);
    }
    if base == mine {
        return clean(theirs);
    }
    let later = Merged { text: mine.into(), conflict: true };
    let (b, m, t) = (tokens(base), tokens(mine), tokens(theirs));
    if [b.len(), m.len(), t.len()].iter().any(|n| *n > MAX_TOKENS) || b.len() * m.len().max(t.len()) > MAX_CELLS {
        return later;
    }
    let (mh, th) = (hunks(&b, &m), hunks(&b, &t));
    let mut conflict = false;
    // Where both changed the same part, mine stays and theirs goes.
    let mut all: Vec<(&Hunk, bool)> = mh.iter().map(|h| (h, true)).collect();
    for h in &th {
        if mh.iter().any(|o| overlaps(h, o)) {
            conflict = true;
        } else {
            all.push((h, false));
        }
    }
    // In base order; at one point, mine first.
    crate::sort_by(&mut all, |(h, mine), (o, theirs)| (h.from, h.to, !mine).cmp(&(o.from, o.to, !theirs)));
    let (mut out, mut at) = (String::new(), 0);
    for (h, _) in all {
        if h.from < at {
            return later;
        }
        out.extend(b[at..h.from].iter().copied().chain(h.insert.iter().copied()));
        at = h.to;
    }
    out.extend(b[at..].iter().copied());
    Merged { text: out, conflict }
}

/// How a copy shows a field its own waiting calls and the server both
/// changed (aio's `sync.merge`), until the server's answer comes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Merge {
    /// Prose: both edits, by [`text3`] (aio's `text`); one part both
    /// changed shows the copy's.
    Text,
    /// A number: both changes added (aio's `counter`).
    Counter,
    /// A list kept as a set: what either side added stays, what the copy
    /// took out goes (aio's `set-add`). Items are the same by this key of
    /// theirs (aio's `identity`), or `None`: whole.
    SetAdd(Option<String>),
    /// A record (an object): each key from the side that changed it, the
    /// copy's where both did, a key either side took out gone (aio's
    /// `lww-per-key`, by key rather than one clock per record).
    Keys,
    /// A list kept as a set: what either side added stays, what either took
    /// out goes, an item one side edited shows edited, the copy's where both
    /// did (aio's `set-remove`). Items are the same by this key, or whole.
    SetRemove(Option<String>),
}

/// `l` against `b` and `r`, one value: the side that changed it, the
/// copy's where both did (its call lands after the server's change).
fn three<'a>(b: Option<&Value>, l: Option<&'a Value>, r: Option<&'a Value>) -> Option<&'a Value> {
    if l == b { r } else { l }
}

/// A set's items by identity (`key`'s value, or whole), in order.
fn by_id<'a>(key: &Option<String>, list: &'a [Value]) -> Vec<(Value, &'a Value)> {
    let id = |x: &Value| match key {
        Some(k) => x.get(k).cloned().unwrap_or(Value::Null),
        None => x.clone(),
    };
    list.iter().map(|x| (id(x), x)).collect()
}

fn find<'a>(list: &[(Value, &'a Value)], id: &Value) -> Option<&'a Value> {
    list.iter().find(|(i, _)| i == id).map(|(_, x)| *x)
}

/// Did the two sides change one key or item each their own way (a
/// [`Conflict`] under [`Merge::Keys`] or [`Merge::SetRemove`])?
fn clash(how: &Merge, b: &Value, l: &Value, r: &Value) -> bool {
    let differ = |b: Option<&Value>, l: Option<&Value>, r: Option<&Value>| l != b && r != b && l != r;
    match (how, b, l, r) {
        (Merge::Keys, Value::Object(b), Value::Object(l), Value::Object(r)) => {
            l.keys().chain(r.keys()).any(|k| differ(b.get(k), l.get(k), r.get(k)))
        }
        (Merge::SetRemove(k), Value::Array(b), Value::Array(l), Value::Array(r)) => {
            let (b, l, r) = (by_id(k, b), by_id(k, l), by_id(k, r));
            b.iter().any(|(id, _)| differ(find(&b, id), find(&l, id), find(&r, id)))
                || l.iter()
                    .any(|(id, _)| find(&b, id).is_none() && find(&r, id).is_some_and(|x| Some(x) != find(&l, id)))
        }
        _ => false,
    }
}

/// A server change that met a change of the copy's own still waiting, the
/// two different (aio's `onConflict`). What the copy shows next is its
/// calls replayed on the server's value (or the field's [`Merge`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    /// Where: `notes.3.title`.
    pub field: String,
    /// What the copy showed.
    pub local: Value,
    /// What the server has.
    pub remote: Value,
}

fn at<'a>(v: &'a Value, path: &[crate::patch::Step]) -> Option<&'a Value> {
    path.iter().try_fold(v, |v, s| match s {
        crate::patch::Step::Key(k) => v.get(k),
        crate::patch::Step::Index(i) => v.get(i),
    })
}

/// Where the server's change (`base` → `remote`, its views) met the copy's
/// own (`base` → `local`, what it showed), the two different; under a
/// field with a [`Merge`], only text both edited in one part (pure).
pub fn conflicts(base: &Value, remote: &Value, local: &Value, merges: &[(String, Merge)]) -> Vec<Conflict> {
    use crate::patch::{Op, Step};
    let mut out: Vec<Conflict> = Vec::new();
    for op in crate::patch::diff(base, remote) {
        let path = match &op {
            // An item put in or taken out moves the rest: the list is the value.
            Op::Insert { path, .. } => &path[..path.len() - 1],
            Op::Del { path } if matches!(path.last(), Some(Step::Index(_))) => &path[..path.len() - 1],
            Op::Set { path, .. } | Op::Del { path } | Op::Append { path, .. } => &path[..],
        };
        let field = |p: &[Step]| {
            p.iter()
                .map(|s| match s {
                    Step::Key(k) => k.clone(),
                    Step::Index(i) => i.to_string(),
                })
                .collect::<Vec<_>>()
                .join(".")
        };
        let how = match path.first() {
            Some(Step::Key(k)) => merges.iter().find(|(f, _)| f == k).map(|(_, m)| m),
            _ => None,
        };
        let path = if how.is_some() { &path[..1] } else { path };
        let (b, r, l) = (at(base, path), at(remote, path), at(local, path));
        let met = match how {
            Some(m @ (Merge::Keys | Merge::SetRemove(_))) => {
                b.zip(l).zip(r).is_some_and(|((b, l), r)| clash(m, b, l, r))
            }
            Some(Merge::Text) => match (b, l, r) {
                (Some(Value::String(b)), Some(Value::String(l)), Some(Value::String(r))) => text3(b, l, r).conflict,
                _ => l != b && l != r,
            },
            Some(_) => false,
            None => l != b && l != r,
        };
        let field = field(path);
        if met && !out.iter().any(|c| c.field == field) {
            let v = |x: Option<&Value>| x.cloned().unwrap_or(Value::Null);
            out.push(Conflict { field, local: v(l), remote: v(r) });
        }
    }
    out
}

/// `field`'s value merged as `how` says, from `base` to `local` (the copy's
/// calls) and to `remote` (the server's); `None`: they don't fit `how`
/// (pure).
pub fn merged(how: &Merge, base: &Value, local: &Value, remote: &Value) -> Option<Value> {
    match (how, base, local, remote) {
        (Merge::Text, Value::String(b), Value::String(l), Value::String(r)) => Some(text3(b, l, r).text.into()),
        (Merge::Counter, b, l, r) if b.is_i64() && l.is_i64() && r.is_i64() => {
            let (b, l, r) = (b.as_i64()?, l.as_i64()?, r.as_i64()?);
            Some(r.checked_add(l.checked_sub(b)?)?.into())
        }
        (Merge::Counter, b, l, r) => {
            Some(serde_json::Number::from_f64(r.as_f64()? + l.as_f64()? - b.as_f64()?)?.into())
        }
        (Merge::SetAdd(key), Value::Array(b), Value::Array(l), Value::Array(r)) => {
            let id = |x: &Value| match key {
                Some(k) => x.get(k).cloned().unwrap_or(Value::Null),
                None => x.clone(),
            };
            let has = |list: &[Value], x: &Value| list.iter().any(|y| id(y) == id(x));
            let mut out: Vec<Value> = r.iter().filter(|x| !(has(b, x) && !has(l, x))).cloned().collect();
            for x in l.iter().filter(|x| !has(b, x)) {
                if !has(&out, x) {
                    out.push(x.clone());
                }
            }
            Some(Value::Array(out))
        }
        (Merge::Keys, Value::Object(b), Value::Object(l), Value::Object(r)) => {
            // Inserted one by one: no sort of the keys (a collect sorts), less code in every app.
            let mut out = serde_json::Map::new();
            for k in b.keys().chain(l.keys()).chain(r.keys()) {
                if let Some(v) = three(b.get(k), l.get(k), r.get(k)) {
                    out.insert(k.clone(), v.clone());
                }
            }
            Some(Value::Object(out))
        }
        (Merge::SetRemove(key), Value::Array(b), Value::Array(l), Value::Array(r)) => {
            let (b, l, r) = (by_id(key, b), by_id(key, l), by_id(key, r));
            // The server's order, then the copy's additions; an item in the base either side took out is gone.
            let ids = r.iter().chain(l.iter().filter(|(id, _)| find(&r, id).is_none())).map(|(id, _)| id);
            let kept = ids.filter_map(|id| match find(&b, id) {
                Some(_) if find(&l, id).is_none() || find(&r, id).is_none() => None,
                was => three(was, find(&l, id), find(&r, id)).cloned(),
            });
            Some(Value::Array(kept.collect()))
        }
        _ => None,
    }
}

/// `shown` (the copy's calls replayed on the server's view) with each
/// field of `merges` that both changed shown merged instead: `base` the
/// server's view its oldest waiting call was made on, `local` the calls
/// replayed on it, `remote` the server's view now (pure).
pub fn show(merges: &[(String, Merge)], base: &Value, local: &Value, remote: &Value, shown: &mut Value) {
    for (f, how) in merges {
        let (Some(b), Some(l), Some(r)) = (base.get(f), local.get(f), remote.get(f)) else { continue };
        if b != r
            && b != l
            && let Some(v) = merged(how, b, l, r)
            && let Some(m) = shown.as_object_mut()
        {
            m.insert(f.clone(), v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_server_change_over_a_waiting_one_is_a_conflict_only_when_they_differ() {
        let base = json!({ "title": "a", "n": 1, "tags": ["x"], "body": "one\ntwo\n" });
        // The server changed the title; the copy too, differently: a conflict.
        let local = json!({ "title": "mine", "n": 1, "tags": ["x"], "body": "one\ntwo\n" });
        let remote = json!({ "title": "theirs", "n": 2, "tags": ["x"], "body": "one\ntwo\n" });
        assert_eq!(
            conflicts(&base, &remote, &local, &[]),
            [Conflict { field: "title".into(), local: json!("mine"), remote: json!("theirs") }]
        );
        // The same change on both sides, or only one side's: none.
        assert!(conflicts(&base, &local, &local, &[]).is_empty());
        assert!(conflicts(&base, &remote, &base, &[]).is_empty());
        // An item put in moves the rest: the list is where they met.
        let (l, r) = (json!({ "tags": ["x", "l"] }), json!({ "tags": ["x", "r"] }));
        let c = conflicts(&json!({ "tags": ["x"] }), &r, &l, &[]);
        assert_eq!(c.iter().map(|c| c.field.as_str()).collect::<Vec<_>>(), ["tags"]);
        // Merged fields: a counter never conflicts; text only in one part both changed.
        let merges = [("n".to_string(), Merge::Counter), ("body".to_string(), Merge::Text)];
        let local = json!({ "n": 5, "body": "ONE\ntwo\n" });
        let remote = json!({ "n": 2, "body": "one\nTWO\n" });
        let base = json!({ "n": 1, "body": "one\ntwo\n" });
        assert!(conflicts(&base, &remote, &local, &merges).is_empty());
        let remote = json!({ "n": 2, "body": "uno\ntwo\n" });
        assert_eq!(conflicts(&base, &remote, &local, &merges)[0].field, "body");
        // By key and as a removing set: only a key or item both changed their own way.
        let merges = [("k".to_string(), Merge::Keys), ("s".to_string(), Merge::SetRemove(None))];
        let base = json!({ "k": { "a": 1, "b": 1 }, "s": [1, 2] });
        let local = json!({ "k": { "a": 2, "b": 1 }, "s": [1, 2, 3] });
        assert!(conflicts(&base, &json!({ "k": { "a": 1, "b": 2 }, "s": [2] }), &local, &merges).is_empty());
        let c = conflicts(&base, &json!({ "k": { "a": 3, "b": 1 }, "s": [1, 2] }), &local, &merges);
        assert_eq!(
            c,
            [Conflict { field: "k".into(), local: json!({ "a": 2, "b": 1 }), remote: json!({ "a": 3, "b": 1 }) }]
        );
    }

    #[test]
    fn a_field_both_changed_shows_merged() {
        let m = |how, b, l, r| merged(&how, &b, &l, &r);
        assert_eq!(m(Merge::Counter, json!(10), json!(15), json!(18)), Some(json!(23)));
        assert_eq!(m(Merge::Counter, json!(1.5), json!(2), json!(1)), Some(json!(1.5)));
        assert_eq!(m(Merge::Counter, json!(i64::MAX), json!(i64::MAX), json!(i64::MAX)), Some(json!(i64::MAX)));
        assert_eq!(m(Merge::Counter, json!("x"), json!(1), json!(2)), None);
        assert_eq!(
            m(Merge::Text, json!("Frist Post"), json!("First Post"), json!("Frist Post!")),
            Some(json!("First Post!"))
        );
        // Add-wins: both sides' additions; what the copy took out goes, by identity.
        let (b, l, r) = (
            json!([{ "id": 1 }, { "id": 2 }, { "id": 3 }]),
            json!([{ "id": 1 }, { "id": 3, "v": "edited" }, { "id": 4 }]),
            json!([{ "id": 1 }, { "id": 2 }, { "id": 3 }, { "id": 5 }]),
        );
        assert_eq!(
            m(Merge::SetAdd(Some("id".into())), b, l, r),
            Some(json!([{ "id": 1 }, { "id": 3 }, { "id": 5 }, { "id": 4 }]))
        );
        assert_eq!(m(Merge::SetAdd(None), json!([1]), json!([1, 2]), json!([3])), Some(json!([3, 2])));
        // By key: each key from the side that changed it, the copy's where both did; taken out is gone.
        assert_eq!(
            m(
                Merge::Keys,
                json!({ "a": 1, "b": 1, "c": 1, "d": 1 }),
                json!({ "a": 2, "b": 1, "c": 5, "e": 1 }),
                json!({ "a": 1, "b": 3, "c": 6, "d": 1, "f": 1 })
            ),
            Some(json!({ "a": 2, "b": 3, "c": 5, "e": 1, "f": 1 }))
        );
        assert_eq!(m(Merge::Keys, json!({}), json!([]), json!({})), None, "not a record");
        // Remove-wins on either side; edits of kept items show; both sides' additions.
        let (b, l, r) = (
            json!([{ "id": 1 }, { "id": 2 }, { "id": 3 }, { "id": 4 }]),
            json!([{ "id": 1, "v": "mine" }, { "id": 3 }, { "id": 4, "v": "mine" }, { "id": 6 }]),
            json!([{ "id": 1 }, { "id": 2 }, { "id": 4, "v": "theirs" }, { "id": 5 }]),
        );
        assert_eq!(
            m(Merge::SetRemove(Some("id".into())), b, l, r),
            Some(json!([{ "id": 1, "v": "mine" }, { "id": 4, "v": "mine" }, { "id": 5 }, { "id": 6 }]))
        );
        // Shown merged only where both changed it.
        let merges = [("n".to_string(), Merge::Counter), ("t".to_string(), Merge::Text)];
        let mut shown = json!({ "n": 15, "t": "b" });
        show(
            &merges,
            &json!({ "n": 10, "t": "a" }),
            &json!({ "n": 15, "t": "a" }),
            &json!({ "n": 18, "t": "b" }),
            &mut shown,
        );
        assert_eq!(shown, json!({ "n": 23, "t": "b" }));
    }

    fn clean(text: &str) -> Merged {
        Merged { text: text.into(), conflict: false }
    }

    #[test]
    fn edits_to_different_parts_both_survive() {
        let base = "one\ntwo\nthree\n";
        assert_eq!(text3(base, "ONE\ntwo\nthree\n", "one\ntwo\nTHREE\n"), clean("ONE\ntwo\nTHREE\n"));
        // Both appended a paragraph at the end: the same point, no order between them.
        let both = text3(base, "one\ntwo\nthree\nmine\n", "one\ntwo\nthree\ntheirs\n");
        assert_eq!(both, Merged { text: "one\ntwo\nthree\nmine\n".into(), conflict: true });
        // One line: by character, so a typo fix and an append both stay.
        assert_eq!(text3("Frist Post", "First Post", "Frist Post!"), clean("First Post!"));
        // An insertion at the edge of a rewrite is no conflict.
        assert_eq!(text3("a\nb\n", "x\na\nb\n", "a\nB\n"), clean("x\na\nB\n"));
        assert_eq!(text3("ünï", "üXnï", "ünïY"), clean("üXnïY"));
    }

    #[test]
    fn one_side_unchanged_or_both_the_same_is_no_merge() {
        assert_eq!(text3("a", "b", "a"), clean("b"));
        assert_eq!(text3("a", "a", "c"), clean("c"));
        assert_eq!(text3("a", "z", "z"), clean("z"));
        assert_eq!(text3("", "", ""), clean(""));
        assert_eq!(text3("", "mine", ""), clean("mine"));
    }

    #[test]
    fn the_same_part_changed_twice_is_a_conflict_the_later_edit_wins() {
        let base = "title\n\nbody\nend\n";
        let m = text3(base, "title\n\nmy body\nend\n", "TITLE\n\ntheir body\nend\n");
        assert_eq!(m, Merged { text: "TITLE\n\nmy body\nend\n".into(), conflict: true });
        // Deterministic: every server computes the same text.
        assert_eq!(text3(base, "title\n\nmy body\nend\n", "TITLE\n\ntheir body\nend\n"), m);
    }

    #[test]
    fn too_big_to_diff_is_the_later_edit_said_as_a_conflict() {
        let line = "x".repeat(MAX_TOKENS + 1);
        let m = text3(&line, &format!("{line}a"), &format!("b{line}"));
        assert_eq!(m, Merged { text: format!("{line}a"), conflict: true });
        assert_eq!(tokens("a\nb"), ["a\n", "b"]);
    }
}

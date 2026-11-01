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
    all.sort_by_key(|(h, mine)| (h.from, h.to, !mine));
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

#[cfg(test)]
mod tests {
    use super::*;

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

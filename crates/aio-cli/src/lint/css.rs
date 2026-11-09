//! A class two stylesheets define differently (aio's `aiol` styles check):
//! two top-level `.track { … }` rules setting one property to two values.
//! Whichever loads last wins, silently. Narrow on purpose: a selector with
//! more to it (`.track:hover`, `.list .track`), a rule inside `@media` and
//! custom properties (`--x`, how theming works) are never compared. A
//! generated sheet (starting `/*!`) is left out. `aio::ui::css` classes
//! can't collide: each is its rule's hash.
use super::{Issue, Severity};

/// One top-level rule naming only classes: where, the class, what it sets.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub file: String,
    pub line: usize,
    pub class: String,
    pub decls: Vec<(String, String)>,
}

/// Every top-level `.a { … }` (or `.a, .b { … }`) rule of `css` (pure).
pub fn rules(file: &str, css: &str) -> Vec<Rule> {
    if css.trim_start().starts_with("/*!") {
        return vec![];
    }
    let src = strip_comments(css);
    let b = src.as_bytes();
    let (mut out, mut depth, mut start) = (vec![], 0usize, 0usize);
    for (i, c) in b.iter().enumerate() {
        match c {
            b'{' => {
                if depth == 0 {
                    let sel = &src[start..i];
                    let parts: Vec<&str> = sel.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
                    let names: Vec<&str> = parts.iter().filter_map(|p| solo_class(p)).collect();
                    if !names.is_empty() && names.len() == parts.len() {
                        let body = block(&src[i + 1..]);
                        let lead = sel.len() - sel.trim_start().len();
                        let line = src[..start + lead].matches('\n').count() + 1;
                        let decls = decls(body);
                        out.extend(names.iter().map(|n| Rule {
                            file: file.into(),
                            line,
                            class: n.to_string(),
                            decls: decls.clone(),
                        }));
                    }
                }
                depth += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    start = i + 1;
                }
            }
            b';' if depth == 0 => start = i + 1,
            _ => {}
        }
    }
    out
}

/// `/* … */` blanked out, its newlines kept (lines stay where they were).
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(at) = rest.find("/*") {
        out.push_str(&rest[..at]);
        let end = rest[at..].find("*/").map_or(rest.len(), |e| at + e + 2);
        out.extend(rest[at..end].chars().map(|c| if c == '\n' { '\n' } else { ' ' }));
        rest = &rest[end..];
    }
    out + rest
}

/// `.name` and nothing else: the name.
fn solo_class(sel: &str) -> Option<&str> {
    let name = sel.strip_prefix('.')?;
    let ok = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !name.trim_start_matches('-').starts_with(|c: char| c.is_ascii_digit());
    ok.then_some(name)
}

/// The text of the block whose `{` was just before `rest`.
fn block(rest: &str) -> &str {
    let mut depth = 1;
    for (i, c) in rest.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &rest[..i];
                }
            }
            _ => {}
        }
    }
    rest
}

/// A rule's plain declarations, its whitespace runs as one space; custom
/// properties and nested rules left out.
fn decls(body: &str) -> Vec<(String, String)> {
    // Before a nested rule: up to its selector's `;`.
    let flat = match body.split_once('{') {
        Some((d, _)) => d.rsplit_once(';').map_or("", |(d, _)| d),
        None => body,
    };
    flat.split(';')
        .filter_map(|d| {
            let (p, v) = d.split_once(':')?;
            let p = p.trim().to_ascii_lowercase();
            let plain = !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c == '-') && !p.starts_with("--");
            plain.then(|| (p, v.split_whitespace().collect::<Vec<_>>().join(" ")))
        })
        .collect()
}

/// Each pair of rules naming one class and disagreeing on a property: the
/// first property they disagree on, as a warning on the later rule.
pub fn collisions(rules: &[Rule]) -> Vec<Issue> {
    let mut out = vec![];
    for (i, a) in rules.iter().enumerate() {
        for b in rules[i + 1..].iter().filter(|b| b.class == a.class) {
            let clash = a.decls.iter().find_map(|(p, av)| {
                let bv = &b.decls.iter().find(|(q, _)| q == p)?.1;
                (bv != av).then_some((p, av, bv))
            });
            if let Some((p, av, bv)) = clash {
                out.push(
                    Issue::new(
                        Severity::Warn,
                        "css",
                        format!(
                            ".{} is defined twice and they disagree on `{p}`: {}:{} says `{av}`, here `{bv}`; whichever \
                             loads last wins, silently",
                            a.class, a.file, a.line
                        ),
                        &b.file,
                        b.line,
                    )
                    .fix(&format!("rename one, make one more specific (`.list .{}`), or use aio::ui::css", a.class)),
                );
            }
        }
    }
    out
}

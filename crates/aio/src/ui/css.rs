//! Scoped CSS (aio's `css`, `collectCss`): `css("color: red; &:hover { … }")`
//! is a class named by a hash of its rule, so two components can't collide
//! however they name things, and the same rule written twice is one class.
//! The rule goes into the page the first time a window uses it (native CSS
//! nesting does the scoping: `&`, `@media` inside); `@keyframes`,
//! `@font-face` and the other at-rules that hold no selectors are lifted
//! out, unscoped. Values are the caller's: `css(&format!("color: {c}"))`
//! is one class per value, as it must be.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

/// This window's rules, by class (each app instance has its own page).
static RULES: air::Global<Rc<RefCell<BTreeMap<String, String>>>> = air::Global::new(Rc::default);

/// The class for `body` (a scoped rule), its rule put in the page once.
pub fn css(body: &str) -> String {
    let cls = format!("aio-{}", hash(&key(body)));
    let rules = RULES.get_untracked();
    if !rules.borrow().contains_key(&cls) {
        let rule = expand(&cls, body);
        let js = serde_json::to_string(&rule).unwrap_or_default();
        air::exec(&format!(
            "let s = document.querySelector('style[data-aio-css]'); if (!s) {{ s = document.createElement('style'); \
             s.setAttribute('data-aio-css', ''); document.head.append(s); }} s.textContent += {js};"
        ));
        rules.borrow_mut().insert(cls.clone(), rule);
    }
    cls
}

/// Every rule [`css`] made in this window, as one stylesheet (`</` escaped,
/// to sit in a `<style>`).
pub fn collect_css() -> String {
    RULES.get_untracked().borrow().values().map(|r| r.replace("</", "\\00003c /")).collect()
}

/// FNV-1a over the rule, in base 36 (aio's `hash`).
fn hash(s: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for u in s.encode_utf16() {
        h ^= u32::from(u);
        h = h.wrapping_mul(0x0100_0193);
    }
    let mut out = vec![];
    loop {
        out.push(char::from_digit(h % 36, 36).unwrap_or('0'));
        h /= 36;
        if h == 0 {
            break;
        }
    }
    out.iter().rev().collect()
}

/// What a rule is the same as: its whitespace runs as one space (a space
/// before `:` stays: `& :first-child` is not `&:first-child`).
fn key(body: &str) -> String {
    body.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `.cls{body}`, with each top-level at-rule that holds no selectors
/// (`@keyframes`, `@font-face`, `@property`, …) lifted out before it.
fn expand(cls: &str, body: &str) -> String {
    let (mut kept, mut lifted) = (String::new(), String::new());
    let b = body.as_bytes();
    let (mut i, mut start) = (0, 0);
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => i = skip_string(b, i),
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i = body[i + 2..].find("*/").map_or(b.len(), |e| i + 2 + e + 2);
            }
            b'@' if at_top(&body[..i]) => {
                let name: String = body[i + 1..].chars().take_while(|c| c.is_alphanumeric() || *c == '-').collect();
                let Some(open) = body[i..].find(['{', ';']).map(|o| i + o) else { break };
                if b[open] == b';' || GROUP.contains(&name.to_ascii_lowercase().as_str()) {
                    i = open + 1;
                    continue;
                }
                let end = block_end(b, open);
                kept.push_str(&body[start..i]);
                lifted.push_str(body[i..end].trim());
                (i, start) = (end, end);
                continue;
            }
            _ => i += 1,
        }
    }
    kept.push_str(&body[start..]);
    format!("{lifted}.{cls}{{{}}}", kept.trim())
}

/// The at-rules whose body is ordinary rules: scoped like the top level.
const GROUP: &[&str] = &["media", "supports", "container", "layer", "scope", "starting-style"];

/// Is the text before `at` at the rule's top level (every block it opened closed)?
fn at_top(before: &str) -> bool {
    let b = before.as_bytes();
    let (mut depth, mut i) = (0i32, 0);
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                i = skip_string(b, i);
                continue;
            }
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    depth == 0
}

/// Past the string that starts at `i`.
fn skip_string(b: &[u8], i: usize) -> usize {
    let q = b[i];
    let mut j = i + 1;
    while j < b.len() && b[j] != q {
        j += if b[j] == b'\\' { 2 } else { 1 };
    }
    (j + 1).min(b.len())
}

/// Past the `}` that closes the block opened at `open`.
fn block_end(b: &[u8], open: usize) -> usize {
    let (mut depth, mut i) = (0, open);
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                i = skip_string(b, i);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    b.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_is_scoped_to_its_hash_and_at_rules_with_no_selectors_are_lifted() {
        assert_eq!((hash(""), hash("color: red")), ("ztntfp".into(), "1sk1uap".into()), "aio's FNV-1a in base 36");
        assert_eq!(key("a  :b\n {c}"), "a :b {c}");
        assert_ne!(hash(&key("& :first-child{}")), hash(&key("&:first-child{}")), "a descendant stays one");
        assert_eq!(
            expand("aio-x", " color: red; &:hover { color: blue } "),
            ".aio-x{color: red; &:hover { color: blue }}"
        );
        assert_eq!(
            expand(
                "aio-x",
                "animation: s 1s; @keyframes s { from { opacity: 0 } } @media (max-width: 9px) { & { a: b } }"
            ),
            "@keyframes s { from { opacity: 0 } }.aio-x{animation: s 1s;  @media (max-width: 9px) { & { a: b } }}"
        );
        assert_eq!(expand("aio-x", "content: \"@x {\"; /* @y { */ b: c"), ".aio-x{content: \"@x {\"; /* @y { */ b: c}");
    }
}

//! Safe Markdown (aio's `Markdown`): parsed into a tree of elements, never an
//! HTML string, so all text is text and raw HTML stays inert. The one
//! attribute the source controls, a link or image target, is checked after
//! the URL parser's own cleanup (http, https, mailto and relative only).
//! A common subset: headings, bold, italic, code, fences, links, images,
//! lists, quotes, rules, paragraphs. Linear in its input, any input.
use super::join;
use crate::ui::prelude::*;

/// Inline content.
#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String),
    Code(String),
    Strong(Vec<Inline>),
    Em(Vec<Inline>),
    /// `external`: http(s), opened outside (`target="_blank"`).
    Link {
        href: String,
        external: bool,
        label: Vec<Inline>,
    },
    Image {
        src: String,
        alt: String,
    },
    Break,
}

/// A block.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading(u8, Vec<Inline>),
    Paragraph(Vec<Inline>),
    Fence { lang: String, text: String },
    Quote(Vec<Block>),
    List { ordered: bool, items: Vec<Vec<Inline>> },
    Rule,
}

/// The target a browser would follow, if it is one this renders: the URL
/// parser drops tab/LF/CR anywhere and C0 controls or spaces at the ends
/// before it reads the scheme, so the check (and the emitted value) are made
/// on that same string. `None`: javascript:, data:, vbscript:, …
pub fn safe_href(url: &str) -> Option<String> {
    let u: String = url.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let u = u.trim_matches(|c: char| c <= ' ');
    let low = u.to_ascii_lowercase();
    if u.is_empty() {
        return None;
    }
    if ["http:", "https:", "mailto:"].iter().any(|s| low.starts_with(s)) || u.starts_with(['.', '/', '#', '?']) {
        return Some(u.into());
    }
    let scheme = u.split_once(':').is_some_and(|(s, _)| {
        s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
    });
    (!scheme).then(|| u.into())
}

fn dropped() {
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| eprintln!("[aio] <Markdown> dropped a link/image target with an unsupported scheme"));
}

/// For each index, the first index at or after it where `hit` holds (n if none):
/// every "where is the next closer" question in O(1), so a line is read once.
fn next_where(t: &[char], hit: impl Fn(usize) -> bool) -> Vec<usize> {
    let mut v = vec![t.len(); t.len() + 1];
    for i in (0..t.len()).rev() {
        v[i] = if hit(i) { i } else { v[i + 1] };
    }
    v
}

fn space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// Inline Markdown in one line (code first, so `*` in code stays literal).
fn inline(t: &[char]) -> Vec<Inline> {
    let n = t.len();
    let at = |i: usize| t.get(i).copied();
    let tick = next_where(t, |i| t[i] == '`');
    let bracket = next_where(t, |i| t[i] == ']');
    let line_end = next_where(t, |i| matches!(t[i], '\n' | '\r' | '\u{2028}' | '\u{2029}'));
    let href_end = next_where(t, |i| t[i] == ')' || space(t[i]));
    let pair = |c: char| next_where(t, move |i| t[i] == c && at(i + 1) == Some(c));
    let (bold_star, bold_under) = (pair('*'), pair('_'));
    // A `*`/`_` that closes is one not after a space.
    let closer = |c: char| next_where(t, move |i| t[i] == c && i > 0 && !space(t[i - 1]));
    let (em_star, em_under) = (closer('*'), closer('_'));
    let next = |v: &[usize], from: usize| (from <= n).then(|| v[from]).filter(|&k| k < n);
    // One or more characters from `start` to `close`, none of them a line end.
    let run = |start: usize, close: usize| close > start && next(&line_end, start).is_none_or(|e| e >= close);
    let href_at = |j: usize| {
        (at(j + 1) == Some('(')).then(|| next(&href_end, j + 2)).flatten().filter(|&e| e > j + 2 && t[e] == ')')
    };
    let s = |a: usize, b: usize| t[a..b].iter().collect::<String>();

    let (mut out, mut plain, mut i) = (Vec::new(), String::new(), 0);
    let flush = |out: &mut Vec<Inline>, plain: &mut String| {
        if !plain.is_empty() {
            out.push(Inline::Text(std::mem::take(plain)));
        }
    };
    while i < n {
        let c = t[i];
        if c == '`'
            && let Some(close) = next(&tick, i + 1).filter(|&k| k > i + 1)
        {
            flush(&mut out, &mut plain);
            out.push(Inline::Code(s(i + 1, close)));
            i = close + 1;
            continue;
        }
        if c == '!'
            && at(i + 1) == Some('[')
            && let Some(j) = next(&bracket, i + 2)
            && let Some(end) = href_at(j)
        {
            flush(&mut out, &mut plain);
            let alt = s(i + 2, j);
            match safe_href(&s(j + 2, end)) {
                Some(src) => out.push(Inline::Image { src, alt }),
                None => {
                    dropped();
                    plain = alt;
                }
            }
            i = end + 1;
            continue;
        }
        if c == '['
            && let Some(j) = next(&bracket, i + 1).filter(|&j| j > i + 1)
            && let Some(end) = href_at(j)
        {
            flush(&mut out, &mut plain);
            let label = inline(&t[i + 1..j]);
            match safe_href(&s(j + 2, end)) {
                Some(href) => {
                    let low = href.to_ascii_lowercase();
                    let external = low.starts_with("http:") || low.starts_with("https:");
                    out.push(Inline::Link { href, external, label });
                }
                None => {
                    dropped();
                    out.extend(label);
                }
            }
            i = end + 1;
            continue;
        }
        if (c == '*' || c == '_') && at(i + 1) == Some(c) {
            let v = if c == '*' { &bold_star } else { &bold_under };
            if let Some(close) = next(v, i + 3).filter(|&k| run(i + 2, k)) {
                flush(&mut out, &mut plain);
                out.push(Inline::Strong(inline(&t[i + 2..close])));
                i = close + 2;
                continue;
            }
        }
        if (c == '*' || c == '_') && at(i + 1).is_some_and(|d| !space(d)) {
            let v = if c == '*' { &em_star } else { &em_under };
            if let Some(close) = next(v, i + 2).filter(|&k| run(i + 1, k)) {
                flush(&mut out, &mut plain);
                out.push(Inline::Em(inline(&t[i + 1..close])));
                i = close + 1;
                continue;
            }
        }
        plain.push(c);
        i += 1;
    }
    flush(&mut out, &mut plain);
    out
}

fn inline_str(s: &str) -> Vec<Inline> {
    inline(&s.chars().collect::<Vec<_>>())
}

/// A code fence opener: 3+ backticks, then an info string with none.
fn fence(line: &str) -> Option<(usize, &str)> {
    let ticks = line.len() - line.trim_start_matches('`').len();
    let info = &line[ticks..];
    (ticks >= 3 && !info.contains('`')).then_some((ticks, info))
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let level = line.len() - line.trim_start_matches('#').len();
    let rest = &line[level..];
    ((1..=6).contains(&level) && rest.starts_with(space)).then(|| (level as u8, rest.trim_start_matches(space)))
}

fn rule(line: &str) -> bool {
    let body = line.trim_end_matches(space);
    ['-', '*', '_'].iter().any(|&c| body.len() >= 3 && body.chars().all(|x| x == c))
}

fn quote(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('>')?;
    Some(rest.strip_prefix(space).unwrap_or(rest))
}

/// A list item's text: `- a`, `* a`, `+ a` or `1. a`, after any indent.
fn item(line: &str, ordered: bool) -> Option<&str> {
    let body = line.trim_start_matches(space);
    let rest = if ordered {
        let digits = body.len() - body.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        body[digits..].strip_prefix('.').filter(|_| digits > 0)?
    } else {
        body.strip_prefix(['-', '*', '+'])?
    };
    rest.starts_with(space).then(|| rest.trim_start_matches(space))
}

/// Past this depth a quote's `>` is text: each level is one more recursion.
const MAX_QUOTE_DEPTH: usize = 32;

fn starts_block(line: &str, depth: usize) -> bool {
    fence(line).is_some()
        || heading(line).is_some()
        || rule(line)
        || (depth < MAX_QUOTE_DEPTH && quote(line).is_some())
        || item(line, false).is_some()
        || item(line, true).is_some()
}

/// Parse Markdown into blocks.
pub fn parse(src: &str) -> Vec<Block> {
    blocks(&src.replace("\r\n", "\n").replace('\r', "\n"), 0)
}

fn blocks(src: &str, depth: usize) -> Vec<Block> {
    let lines: Vec<&str> = src.split('\n').collect();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
        } else if let Some((ticks, info)) = fence(line) {
            // Closed by a fence at least as long; an open one runs to the end.
            let lang = info.split_whitespace().next().unwrap_or("").into();
            let body = lines[i + 1..]
                .iter()
                .take_while(|l| fence(l.trim_end_matches(space)).is_none_or(|(t, rest)| t < ticks || !rest.is_empty()))
                .count();
            out.push(Block::Fence { lang, text: lines[i + 1..i + 1 + body].join("\n") });
            i += body + 2;
        } else if let Some((level, text)) = heading(line) {
            out.push(Block::Heading(level, inline_str(text.trim())));
            i += 1;
        } else if rule(line) {
            out.push(Block::Rule);
            i += 1;
        } else if depth < MAX_QUOTE_DEPTH && quote(line).is_some() {
            let body: Vec<&str> = lines[i..].iter().map_while(|l| quote(l)).collect();
            i += body.len();
            out.push(Block::Quote(blocks(&body.join("\n"), depth + 1)));
        } else if let Some(ordered) = [false, true].into_iter().find(|&o| item(line, o).is_some()) {
            let items: Vec<Vec<Inline>> = lines[i..].iter().map_while(|l| item(l, ordered)).map(inline_str).collect();
            i += items.len();
            out.push(Block::List { ordered, items });
        } else {
            // This line, then each next one that is not blank and starts no block;
            // a soft line break is a <br>.
            let more = lines[i + 1..].iter().take_while(|l| !l.trim().is_empty() && !starts_block(l, depth)).count();
            let mut p = Vec::new();
            for (k, l) in lines[i..=i + more].iter().enumerate() {
                if k > 0 {
                    p.push(Inline::Break);
                }
                p.extend(inline_str(l));
            }
            out.push(Block::Paragraph(p));
            i += more + 1;
        }
    }
    out
}

fn inlines(xs: &[Inline]) -> View {
    rsx! {
        for x in xs {
            match x {
                Inline::Text(s) => rsx! { "{s}" },
                Inline::Code(s) => rsx! { code { class: "aio-md__code", "{s}" } },
                Inline::Strong(c) => rsx! { strong { {inlines(c)} } },
                Inline::Em(c) => rsx! { em { {inlines(c)} } },
                Inline::Link { href, external: true, label } => rsx! {
                    a { href: "{href}", class: "aio-md__a", target: "_blank", rel: "noopener noreferrer", {inlines(label)} }
                },
                Inline::Link { href, label, .. } => rsx! { a { href: "{href}", class: "aio-md__a", {inlines(label)} } },
                Inline::Image { src, alt } => rsx! { img { src: "{src}", alt: "{alt}", class: "aio-md__img" } },
                Inline::Break => rsx! { br {} },
            }
        }
    }
}

fn render(bs: &[Block]) -> View {
    rsx! {
        for b in bs {
            match b {
                Block::Heading(1, c) => rsx! { h1 { {inlines(c)} } },
                Block::Heading(2, c) => rsx! { h2 { {inlines(c)} } },
                Block::Heading(3, c) => rsx! { h3 { {inlines(c)} } },
                Block::Heading(4, c) => rsx! { h4 { {inlines(c)} } },
                Block::Heading(5, c) => rsx! { h5 { {inlines(c)} } },
                Block::Heading(_, c) => rsx! { h6 { {inlines(c)} } },
                Block::Paragraph(c) => rsx! { p { class: "aio-md__p", {inlines(c)} } },
                Block::Fence { lang, text } => rsx! {
                    pre { class: "aio-md__pre",
                        code { "data-lang": (!lang.is_empty()).then(|| lang.clone()), "{text}" }
                    }
                },
                Block::Quote(c) => rsx! { blockquote { class: "aio-md__quote", {render(c)} } },
                Block::List { ordered: true, items } => rsx! {
                    ol { class: "aio-md__list",
                        for it in items {
                            li { {inlines(it)} }
                        }
                    }
                },
                Block::List { items, .. } => rsx! {
                    ul { class: "aio-md__list",
                        for it in items {
                            li { {inlines(it)} }
                        }
                    }
                },
                Block::Rule => rsx! { hr { class: "aio-md__hr" } },
            }
        }
    }
}

/// `source` as safe elements.
#[component]
pub fn Markdown(
    #[props(into)] source: String,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    rsx! {
        div { class: join("aio-md", &class), ..attributes, {render(&parse(&source))} }
    }
}

#[cfg(test)]
mod tests {
    use super::Block::*;
    use super::Inline::*;
    use super::{Block, Inline, MAX_QUOTE_DEPTH, parse, safe_href};

    fn text(s: &str) -> Inline {
        Text(s.into())
    }

    #[test]
    fn headings_bold_italic_and_code() {
        assert_eq!(
            parse("# Title\n\nHello **bold** and *italic* and `co*de*`."),
            [
                Heading(1, vec![text("Title")]),
                Paragraph(vec![
                    text("Hello "),
                    Strong(vec![text("bold")]),
                    text(" and "),
                    Em(vec![text("italic")]),
                    text(" and "),
                    Code("co*de*".into()),
                    text("."),
                ]),
            ]
        );
        assert_eq!(parse("a * b * c"), [Paragraph(vec![text("a * b * c")])], "a spaced star is no emphasis");
        assert_eq!(parse("####### x"), [Paragraph(vec![text("####### x")])]);
    }

    #[test]
    fn a_fence_is_literal_and_closed_by_one_as_long() {
        assert_eq!(
            parse("```js\nconst x = **no**;\n```"),
            [Fence { lang: "js".into(), text: "const x = **no**;".into() }]
        );
        assert_eq!(parse("```js title=\"x\"\na\n```")[0], Fence { lang: "js".into(), text: "a".into() });
        assert_eq!(parse("````\n```\n````")[0], Fence { lang: "".into(), text: "```".into() });
        assert_eq!(
            parse("```\nopen"),
            [Fence { lang: "".into(), text: "open".into() }],
            "an open fence runs to the end"
        );
        assert_eq!(parse("```a`b"), [Paragraph(vec![text("``"), Code("a".into()), text("b")])]);
    }

    #[test]
    fn lists_quotes_and_rules() {
        assert_eq!(parse("- one\n- two"), [List { ordered: false, items: vec![vec![text("one")], vec![text("two")]] }]);
        assert_eq!(
            parse("1. first\n2. second"),
            [List { ordered: true, items: vec![vec![text("first")], vec![text("second")]] }]
        );
        assert_eq!(parse("> quoted\n---"), [Quote(vec![Paragraph(vec![text("quoted")])]), Rule]);
        assert_eq!(parse("a\nb"), [Paragraph(vec![text("a"), Break, text("b")])]);
        assert_eq!(parse("a\n# b"), [Paragraph(vec![text("a")]), Heading(1, vec![text("b")])]);
        assert_eq!(parse("# a\u{2028}b"), [Heading(1, vec![text("a\u{2028}b")])], "U+2028 stays in its block");
    }

    #[test]
    fn quotes_nest_up_to_a_cap_and_the_rest_is_text() {
        let deep = ">".repeat(20_000) + " x";
        let mut b = parse(&deep);
        let mut depth = 0;
        while let [Quote(inner)] = b.as_slice() {
            b = inner.clone();
            depth += 1;
        }
        assert_eq!(depth, MAX_QUOTE_DEPTH);
        assert!(matches!(b.as_slice(), [Paragraph(_)]));
    }

    #[test]
    fn links_open_outside_only_when_external() {
        let [Paragraph(p)]: [Block; 1] = parse("[site](https://example.com) [home](/home)").try_into().unwrap() else {
            panic!()
        };
        assert_eq!(p[0], Link { href: "https://example.com".into(), external: true, label: vec![text("site")] });
        assert_eq!(p[2], Link { href: "/home".into(), external: false, label: vec![text("home")] });
        let [Paragraph(q)]: [Block; 1] = parse("[x](https://e.com/a\"onmouseover=\"alert(1))").try_into().unwrap()
        else {
            panic!()
        };
        assert!(
            matches!(&q[0], Link { href, .. } if href.contains("onmouseover")),
            "a quote is part of the href: {q:?}"
        );
    }

    #[test]
    fn an_unsafe_target_is_dropped_and_its_text_kept() {
        assert_eq!(parse("[click](javascript:alert(1))"), [Paragraph(vec![text("click"), text(")")])]);
        assert_eq!(parse("![x](data:text/html,<script>alert(1)</script>)")[0], Paragraph(vec![text("x</script>)")]));
        for prefix in (0..=0x20u8).map(|c| (c as char).to_string()).chain(["".into(), "  ".into()]) {
            for target in ["javascript:alert(1)", "JaVaScRiPt:alert(1)", "data:text/html,x", "vbscript:msgbox(1)"] {
                let raw = format!("{prefix}{target}");
                assert_eq!(safe_href(&raw), None, "{raw:?}");
            }
        }
        assert_eq!(
            safe_href("\0https://ok.test/a\u{1}"),
            Some("https://ok.test/a".into()),
            "the emitted href is the checked one"
        );
        assert_eq!(safe_href("java\tscript:x"), None, "a tab inside the scheme is dropped by the parser");
        assert_eq!(safe_href("example.com/x"), Some("example.com/x".into()));
        assert_eq!(safe_href("mailto:a@b.c"), Some("mailto:a@b.c".into()));
    }

    #[test]
    fn adversarial_input_parses_in_linear_time() {
        let t = std::time::Instant::now();
        for src in ["*a ".repeat(40_000), "[a](".repeat(40_000), "**a ".repeat(40_000), "`".repeat(80_000)] {
            parse(&src);
        }
        assert!(t.elapsed() < std::time::Duration::from_secs(5), "{:?}", t.elapsed());
    }
}

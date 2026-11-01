//! A tiny syntax highlighter for the file viewer (amui's `ui/highlight.tsx`):
//! a tokenizer, not a parser, good enough to read code. Comments, strings,
//! numbers and keywords for Rust, JS/TS and TOML; keys, strings and literals
//! for JSON. Everything else is plain, and a huge file stays plain (amui's
//! perf cap) but still shows.

/// Above this many bytes a file is shown plain.
pub const HIGHLIGHTED: usize = 150_000;

const KEYWORDS: &[&str] = &[
    // Rust
    "as",
    "async",
    "await",
    "break",
    "const",
    "continue",
    "crate",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "return",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
    // JS/TS (amui's list)
    "var",
    "function",
    "import",
    "export",
    "from",
    "new",
    "class",
    "extends",
    "implements",
    "interface",
    "of",
    "try",
    "catch",
    "finally",
    "throw",
    "typeof",
    "instanceof",
    "this",
    "null",
    "undefined",
    "void",
    "default",
    "case",
    "switch",
    "do",
    "yield",
    "public",
    "private",
    "protected",
    "readonly",
    "get",
    "set",
    "namespace",
    "declare",
    "abstract",
    "keyof",
    "satisfies",
];

/// `code` from file `file` as runs of text and their kind (`comment`,
/// `string`, `number`, `keyword`, `key`; `None` plain) (pure). Never a lost
/// character: the runs joined are `code`.
pub fn highlight(code: &str, file: &str) -> Vec<(Option<&'static str>, String)> {
    let name = file.to_ascii_lowercase();
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
    let (slashes, hash, json) = match ext {
        "rs" | "js" | "ts" | "tsx" | "jsx" | "mjs" | "cjs" => (true, false, false),
        "toml" => (false, true, false),
        "json" | "jsonc" => (ext == "jsonc", false, true),
        _ => return vec![(None, code.into())],
    };
    if code.len() > HIGHLIGHTED {
        return vec![(None, code.into())];
    }
    let c: Vec<char> = code.chars().collect();
    let mut out: Vec<(Option<&'static str>, String)> = Vec::new();
    let mut push = |kind, text: String| match out.last_mut() {
        Some((k, t)) if *k == kind => t.push_str(&text),
        _ => out.push((kind, text)),
    };
    let mut i = 0;
    while i < c.len() {
        let start = i;
        let rest = |n: usize| c.get(i + n).copied().unwrap_or('\0');
        let kind = match c[i] {
            '/' if slashes && rest(1) == '/' => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
                Some("comment")
            }
            '#' if hash => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
                Some("comment")
            }
            '/' if slashes && rest(1) == '*' => {
                i += 2;
                while i < c.len() && !(c[i] == '*' && c.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i = (i + 2).min(c.len());
                Some("comment")
            }
            q @ ('"' | '`' | '\'') if q != '\'' || ext != "rs" || rest(2) == '\'' || rest(1) == '\\' => {
                i += 1;
                while i < c.len() && c[i] != q {
                    i += if c[i] == '\\' { 2 } else { 1 };
                }
                i = (i + 1).min(c.len());
                // In JSON a string before `:` is a key.
                let after = c[i..].iter().find(|ch| !ch.is_whitespace());
                Some(if json && after == Some(&':') { "key" } else { "string" })
            }
            '-' if json && rest(1).is_ascii_digit() => {
                i += 1;
                while i < c.len() && (c[i].is_ascii_alphanumeric() || matches!(c[i], '.' | '+' | '-')) {
                    i += 1;
                }
                Some("number")
            }
            d if d.is_ascii_digit() => {
                while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '.' || c[i] == '_') {
                    i += 1;
                }
                Some("number")
            }
            w if w.is_alphabetic() || w == '_' || w == '$' => {
                while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '$') {
                    i += 1;
                }
                let word: String = c[start..i].iter().collect();
                let lit = matches!(word.as_str(), "true" | "false" | "null");
                (if json { lit } else { KEYWORDS.contains(&word.as_str()) }).then_some("keyword")
            }
            _ => {
                i += 1;
                None
            }
        };
        push(kind, c[start..i].iter().collect());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kinds and texts of the runs, plain ones left out.
    fn marked(code: &str, file: &str) -> Vec<(&'static str, String)> {
        highlight(code, file).into_iter().filter_map(|(k, t)| Some((k?, t))).collect()
    }

    /// amui's `highlight: colors TS/JSON, leaves unknown + huge files plain`,
    /// with Rust and TOML besides.
    #[test]
    fn code_is_coloured_by_kind_and_unknown_or_huge_files_stay_plain() {
        let rs = marked("// hi\nfn main() { let s = \"a \\\" b\"; let c = '\"'; f::<'a>(1_000) }", "src/main.rs");
        assert_eq!(
            rs,
            [
                ("comment", "// hi".into()),
                ("keyword", "fn".into()),
                ("keyword", "let".into()),
                ("string", "\"a \\\" b\"".into()),
                ("keyword", "let".into()),
                ("string", "'\"'".into()),
                ("number", "1_000".into()),
            ]
        );
        let toml = marked("# c\nid = \"x\" # tail\nn = 3", "aio.toml");
        assert_eq!(
            toml,
            [
                ("comment", "# c".into()),
                ("string", "\"x\"".into()),
                ("comment", "# tail".into()),
                ("number", "3".into())
            ]
        );
        // amui: a JSON string before `:` is a key; true/false/null literals.
        assert_eq!(
            marked(r#"{"k": true, "s": "v", "n": -3}"#, "a.json"),
            [
                ("key", "\"k\"".into()),
                ("keyword", "true".into()),
                ("key", "\"s\"".into()),
                ("string", "\"v\"".into()),
                ("key", "\"n\"".into()),
                ("number", "-3".into())
            ]
        );
        assert_eq!(
            marked("const x = 'hi'; /* c */ interface", "a.ts"),
            [
                ("keyword", "const".into()),
                ("string", "'hi'".into()),
                ("comment", "/* c */".into()),
                ("keyword", "interface".into())
            ]
        );
        assert_eq!(highlight("fn x", "notes.xyz"), [(None, "fn x".to_string())], "an unknown kind stays plain");
        let big = "fn ".repeat(HIGHLIGHTED);
        assert_eq!(highlight(&big, "a.rs").len(), 1, "a huge file stays plain");
        let all: String = highlight("let s = \"open", "a.rs").into_iter().map(|(_, t)| t).collect();
        assert_eq!(all, "let s = \"open", "never a lost character");
    }
}

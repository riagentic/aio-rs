//! Text the page fetches made smaller, as the app builds (`const`): a
//! stylesheet without comments and needless spaces ([`css`]), a script
//! without indentation and comment lines ([`js`]). Each removes ASCII
//! bytes only, so its result is UTF-8 as its source was.

/// `src` as CSS without comments or needless whitespace into `out`, as
/// much as fits; returns the whole length (an empty `out` counts it).
/// Quoted strings are kept as written; a space is kept where it may
/// matter (between words, before `(` and `:`, around `+` and `-`).
pub const fn css(src: &[u8], out: &mut [u8]) -> usize {
    let (mut i, mut n, mut space, mut quote, mut last) = (0, 0, false, 0u8, 0u8);
    while i < src.len() {
        let c = src[i];
        i += 1;
        if quote != 0 {
            n = put(out, n, c);
            if c == b'\\' && i < src.len() {
                n = put(out, n, src[i]);
                i += 1;
            } else if c == quote {
                quote = 0;
            }
            continue;
        }
        if c == b'/' && i < src.len() && src[i] == b'*' {
            i += 1;
            while i < src.len() && !(src[i - 1] == b'*' && src[i] == b'/') {
                i += 1;
            }
            (i, space) = (i + 1, true);
            continue;
        }
        if c.is_ascii_whitespace() {
            space = true;
            continue;
        }
        let cut_after = matches!(last, 0 | b'{' | b'}' | b';' | b',' | b'>' | b'(' | b':');
        let cut_before = matches!(c, b'{' | b'}' | b';' | b',' | b'>' | b')' | b'!');
        if space && !cut_after && !cut_before {
            n = put(out, n, b' ');
        }
        if c == b'"' || c == b'\'' {
            quote = c;
        }
        (n, space, last) = (put(out, n, c), false, c);
    }
    n
}

/// `src` as JavaScript without each line's indentation and without lines
/// that are only a `//` comment, into `out` as much as fits; returns the
/// whole length (an empty `out` counts it). Lines stay lines (no
/// semicolon is ever inferred differently). `None`: a line ends inside a
/// template literal, whose spaces are text: `src` is left as it is.
pub const fn js(src: &[u8], out: &mut [u8]) -> Option<usize> {
    let (mut i, mut n, mut ticks) = (0, 0, 0usize);
    while i < src.len() {
        while i < src.len() && (src[i] == b' ' || src[i] == b'\t') {
            i += 1;
        }
        let comment = i + 1 < src.len() && src[i] == b'/' && src[i + 1] == b'/';
        while i < src.len() && src[i] != b'\n' {
            if !comment {
                if src[i] == b'`' && (i == 0 || src[i - 1] != b'\\') {
                    ticks += 1;
                }
                n = put(out, n, src[i]);
            }
            i += 1;
        }
        if ticks % 2 == 1 {
            return None;
        }
        if i < src.len() {
            if !comment {
                n = put(out, n, b'\n');
            }
            i += 1;
        }
    }
    Some(n)
}

const fn put(out: &mut [u8], n: usize, c: u8) -> usize {
    if n < out.len() {
        out[n] = c;
    }
    n + 1
}

/// `$src` (a `&str` constant) through [`css`] or [`js`] as the app builds:
/// a `&'static str` holding only the smaller text.
#[doc(hidden)]
#[macro_export]
macro_rules! shrunk {
    (css, $src:expr) => {{
        const N: usize = $crate::shrink::css($src.as_bytes(), &mut []);
        const A: [u8; N] = {
            let mut a = [0u8; N];
            $crate::shrink::css($src.as_bytes(), &mut a);
            a
        };
        match ::core::str::from_utf8(&A) {
            Ok(s) => s,
            Err(_) => panic!("shrink: not UTF-8"),
        }
    }};
    (js, $src:expr) => {{
        const N: usize = match $crate::shrink::js($src.as_bytes(), &mut []) {
            Some(n) => n,
            None => panic!("shrink: a template literal spans lines"),
        };
        const A: [u8; N] = {
            let mut a = [0u8; N];
            _ = $crate::shrink::js($src.as_bytes(), &mut a);
            a
        };
        match ::core::str::from_utf8(&A) {
            Ok(s) => s,
            Err(_) => panic!("shrink: not UTF-8"),
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    fn css_of(s: &str) -> String {
        let mut out = vec![0; css(s.as_bytes(), &mut [])];
        assert_eq!(css(s.as_bytes(), &mut out), out.len());
        String::from_utf8(out).unwrap()
    }

    fn js_of(s: &str) -> Option<String> {
        let mut out = vec![0; js(s.as_bytes(), &mut [])?];
        js(s.as_bytes(), &mut out)?;
        Some(String::from_utf8(out).unwrap())
    }

    #[test]
    fn css_loses_comments_and_spaces_never_meaning() {
        let src = "/* a */\n.a > .b ,\n.c :hover {\n  color : red ;\n  content: \"x  /* y */ }\";\n}\n\
                   @media (max-width: 9px) and (hover: none) { .d { margin: calc(1px + 2px) !important } }\n";
        assert_eq!(
            css_of(src),
            ".a>.b,.c :hover{color :red;content:\"x  /* y */ }\";}@media (max-width:9px) and (hover:none){.d{margin:calc(1px + 2px)!important}}"
        );
        // The count without an `out` is the length written with one.
        assert_eq!(css(src.as_bytes(), &mut []), css_of(src).len());
        assert_eq!(css_of("  /* only */  "), "");
    }

    #[cfg(feature = "kit")]
    #[test]
    fn the_kit_sheet_and_airs_interpreter_shrink_as_the_app_builds() {
        let min = shrunk!(css, crate::kit::CSS);
        assert!(min.len() * 4 < crate::kit::CSS.len() * 3, "{} of {}", min.len(), crate::kit::CSS.len());
        assert_eq!(min.matches('{').count(), min.matches('}').count(), "every block closed");
        assert!(min.contains(".aio-btn{") && !min.contains("/*") && !min.contains('\n'));
        let air = shrunk!(js, air::PAGE_JS);
        assert!(air.len() < air::PAGE_JS.len() * 9 / 10, "{} of {}", air.len(), air::PAGE_JS.len());
    }

    #[test]
    fn js_loses_indentation_and_comment_lines_never_lines() {
        let src = "// head\nfunction f() {\n    // inside\n    return 1 // kept\n    + `a b`\n}\n";
        assert_eq!(js_of(src).unwrap(), "function f() {\nreturn 1 // kept\n+ `a b`\n}\n");
        assert_eq!(js_of("const t = `a\n  b`;\n"), None, "a template literal over lines is text");
        assert_eq!(js_of("x\\`y\n"), Some("x\\`y\n".into()), "an escaped backtick opens none");
    }
}

//! The little of URLs the shell needs, as WHATWG URL parsers read it (pure):
//! a web or mail link checked and written out escaped, a page's origin, and
//! a path as a `file:` URI. An international host is written in punycode
//! (lowercased, then `xn--…`; no further UTS 46 mapping).

/// An absolute `http(s)` URL, taken apart.
#[derive(Debug, PartialEq)]
pub(crate) struct Web {
    scheme: &'static str,
    /// `user:pass@`, kept as written.
    user: String,
    host: String,
    /// `None`: the scheme's own (80, 443).
    port: Option<u16>,
    /// The path, query and fragment, escaped.
    rest: String,
}

impl Web {
    /// `s` if it is an absolute http(s) URL with a host.
    pub(crate) fn parse(s: &str) -> Option<Self> {
        let s = s.trim_matches(|c: char| c <= ' ');
        let (scheme, after) = scheme(s)?;
        let scheme = match scheme.as_str() {
            "http" => "http",
            "https" => "https",
            _ => return None,
        };
        // A special scheme takes `\` for `/`, as browsers do.
        let after = after.strip_prefix(['/', '\\'])?.strip_prefix(['/', '\\'])?;
        let end = after.find(['/', '\\', '?', '#']).unwrap_or(after.len());
        let (authority, rest) = after.split_at(end);
        let (user, hostport) = authority.rsplit_once('@').map_or(("", authority), |(u, h)| (u, h));
        let (host, port) = match hostport.strip_prefix('[') {
            Some(v6) => {
                let (ip, tail) = v6.split_once(']')?;
                ip.parse::<std::net::Ipv6Addr>().ok()?;
                (format!("[{}]", ip.to_ascii_lowercase()), tail.strip_prefix(':'))
            }
            None => match hostport.rsplit_once(':') {
                Some((h, p)) => (host(h)?, Some(p)),
                None => (host(hostport)?, None),
            },
        };
        let port = match port.filter(|p| !p.is_empty()) {
            Some(p) if p.bytes().all(|b| b.is_ascii_digit()) => Some(p.parse::<u16>().ok()?),
            Some(_) => return None,
            None => None,
        };
        let default = if scheme == "http" { 80 } else { 443 };
        Some(Self {
            scheme,
            user: user.to_string(),
            host,
            port: port.filter(|&p| p != default),
            rest: rest_escaped(&rest.replace('\\', "/")),
        })
    }

    /// `scheme://host[:port]`, as `URL.origin` says it.
    pub(crate) fn origin(&self) -> String {
        match self.port {
            Some(p) => format!("{}://{}:{p}", self.scheme, self.host),
            None => format!("{}://{}", self.scheme, self.host),
        }
    }
}

impl std::fmt::Display for Web {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let user = if self.user.is_empty() { String::new() } else { format!("{}@", self.user) };
        let at = self.origin();
        let host = &at[self.scheme.len() + 3..];
        let path = if self.rest.starts_with('/') { "" } else { "/" };
        write!(f, "{}://{user}{host}{path}{}", self.scheme, self.rest)
    }
}

/// A link the OS may open: http(s) or mailto, written out escaped; else why not.
pub(crate) fn link(s: &str) -> Result<String, String> {
    let t = s.trim_matches(|c: char| c <= ' ');
    let (scheme, after) = scheme(t).ok_or_else(|| format!("{s:?}: not a URL"))?;
    match scheme.as_str() {
        "http" | "https" => Web::parse(t).map(|w| w.to_string()).ok_or_else(|| format!("{s:?}: not a web address")),
        "mailto" => Ok(format!("mailto:{}", escape(after.as_bytes(), |b| !(b'!'..=b'~').contains(&b)))),
        other => Err(format!("refused to open a {other}: link")),
    }
}

/// `path` (an absolute path's bytes; `windows`: as Windows writes them) as
/// a `file:` URI.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn file_uri(path: &[u8], windows: bool) -> String {
    let seg = |b: u8| b <= b' ' || b >= 0x7f || b"\"#<>?`{}%".contains(&b);
    if !windows {
        return format!("file://{}", escape(path, seg));
    }
    let p = String::from_utf8_lossy(path).replace('\\', "/");
    let p = p.strip_prefix("//?/UNC/").map(|u| format!("//{u}")).unwrap_or(p);
    let p = p.strip_prefix("//?/").unwrap_or(&p);
    match p.strip_prefix("//") {
        // `\\server\share\x`: the server is the URI's host.
        Some(unc) => format!("file://{}", escape(unc.as_bytes(), seg)),
        None => format!("file:///{}", escape(p.as_bytes(), seg)),
    }
}

/// `s`'s scheme, lowercased, and what follows its colon.
fn scheme(s: &str) -> Option<(String, &str)> {
    let (scheme, after) = s.split_once(':')?;
    let mut b = scheme.bytes();
    let ok = b.next().is_some_and(|c| c.is_ascii_alphabetic())
        && b.all(|c| c.is_ascii_alphanumeric() || b"+-.".contains(&c));
    ok.then(|| (scheme.to_ascii_lowercase(), after))
}

/// A host as it is sent: lowercased, each international label in punycode;
/// `None` for an empty one or one with a character no host has.
fn host(h: &str) -> Option<String> {
    let bad = |c: char| c <= ' ' || "#%/:<>?@[\\]^|\u{7f}".contains(c);
    if h.is_empty() || h.contains(bad) {
        return None;
    }
    let labels: Option<Vec<String>> = h
        .to_lowercase()
        .split('.')
        .map(|l| if l.is_ascii() { Some(l.to_string()) } else { punycode(l).map(|p| format!("xn--{p}")) })
        .collect();
    Some(labels?.join("."))
}

/// The path, query and fragment with what may not stand in each escaped
/// (WHATWG's percent-encode sets; a `%` already there stays).
fn rest_escaped(rest: &str) -> String {
    let control = |b: u8| !(b' '..=b'~').contains(&b);
    let (before_frag, frag) = rest.split_once('#').map_or((rest, None), |(a, f)| (a, Some(f)));
    let (path, query) = before_frag.split_once('?').map_or((before_frag, None), |(p, q)| (p, Some(q)));
    let mut out = escape(path.as_bytes(), |b| control(b) || b" \"<>`{}".contains(&b));
    if let Some(q) = query {
        out += &format!("?{}", escape(q.as_bytes(), |b| control(b) || b" \"<>'".contains(&b)));
    }
    if let Some(f) = frag {
        out += &format!("#{}", escape(f.as_bytes(), |b| control(b) || b" \"<>`".contains(&b)));
    }
    out
}

fn escape(bytes: &[u8], encode: impl Fn(u8) -> bool) -> String {
    bytes.iter().fold(String::new(), |mut s, &b| {
        if encode(b) {
            s += &format!("%{b:02X}");
        } else {
            s.push(b as char);
        }
        s
    })
}

/// RFC 3492's encoding of one label (after its `xn--`).
fn punycode(label: &str) -> Option<String> {
    const BASE: u32 = 36;
    let adapt = |mut delta: u32, points: u32, first: bool| {
        delta = if first { delta / 700 } else { delta / 2 };
        delta += delta / points;
        let mut k = 0;
        while delta > ((BASE - 1) * 26) / 2 {
            delta /= BASE - 1;
            k += BASE;
        }
        k + (BASE * delta) / (delta + 38)
    };
    let digit = |d: u32| char::from(if d < 26 { b'a' + d as u8 } else { b'0' + (d - 26) as u8 });
    let input: Vec<u32> = label.chars().map(u32::from).collect();
    let mut out: String = label.chars().filter(char::is_ascii).collect();
    let basic = out.len() as u32;
    if basic > 0 {
        out.push('-');
    }
    let (mut n, mut delta, mut bias, mut h) = (0x80u32, 0u32, 72u32, basic);
    while (h as usize) < input.len() {
        let m = *input.iter().filter(|&&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(h + 1)?)?;
        n = m;
        for &c in &input {
            if c < n {
                delta = delta.checked_add(1)?;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias {
                        1
                    } else if k >= bias + 26 {
                        26
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    out.push(digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                out.push(digit(q));
                bias = adapt(delta, h + 1, h == basic);
                delta = 0;
                h += 1;
            }
        }
        delta += 1;
        n += 1;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_addresses_read_as_browsers_read_them() {
        let w = |s: &str| Web::parse(s).map(|w| (w.to_string(), w.origin()));
        let both = |a: &str, o: &str| Some((a.to_string(), o.to_string()));
        for (s, want) in [
            ("https://id.example/login", both("https://id.example/login", "https://id.example")),
            ("HTTP://H/x y", both("http://h/x%20y", "http://h")),
            ("https://h/?a=1&b=2", both("https://h/?a=1&b=2", "https://h")),
            ("http://b.example:8080", both("http://b.example:8080/", "http://b.example:8080")),
            ("https://a.example:443/p", both("https://a.example/p", "https://a.example")),
            ("http://u:p@h.example/", both("http://u:p@h.example/", "http://h.example")),
            ("https://[::1]:9/x", both("https://[::1]:9/x", "https://[::1]:9")),
            ("https://h/a\\b?q=\"x\"#f g", both("https://h/a/b?q=%22x%22#f%20g", "https://h")),
            (
                "https://evil.example\\@good.example/",
                both("https://evil.example/@good.example/", "https://evil.example"),
            ),
            ("https://MÜNCHEN.de/", both("https://xn--mnchen-3ya.de/", "https://xn--mnchen-3ya.de")),
            ("https://bücher.example/ü", both("https://xn--bcher-kva.example/%C3%BC", "https://xn--bcher-kva.example")),
            ("  https://h/  ", both("https://h/", "https://h")),
        ] {
            assert_eq!(w(s), want, "{s}");
        }
        for s in [
            "https://",
            "https:///x",
            "https://h:99999/",
            "https://h:8x/",
            "https://a b/",
            "https:h",
            "aio://app/",
            "https://[zz]/",
        ] {
            assert_eq!(w(s), None, "{s}");
        }
    }

    #[test]
    fn only_web_and_mail_links_pass() {
        assert_eq!(link("mailto:a@b.c").unwrap(), "mailto:a@b.c");
        assert_eq!(link("MAILTO:a@b.c?subject=hi there").unwrap(), "mailto:a@b.c?subject=hi%20there");
        assert_eq!(link("https://h/?a=1&b=2").unwrap(), "https://h/?a=1&b=2");
        assert_eq!(link("file:///etc/passwd").unwrap_err(), "refused to open a file: link");
        assert_eq!(link("JavaScript:alert(1)").unwrap_err(), "refused to open a javascript: link");
        assert!(link("not a url").unwrap_err().contains("not a URL"));
        assert!(link("https://").unwrap_err().contains("not a web address"));
    }

    #[test]
    fn paths_become_file_uris_on_each_os() {
        for (p, windows, want) in [
            ("/home/u/a,b c.txt", false, "file:///home/u/a,b%20c.txt"),
            ("/tmp/100%/#1?.txt", false, "file:///tmp/100%25/%231%3F.txt"),
            ("/srv/ü", false, "file:///srv/%C3%BC"),
            (r"C:\Users\u\a b.txt", true, "file:///C:/Users/u/a%20b.txt"),
            (r"\\server\share\x y", true, "file://server/share/x%20y"),
            (r"\\?\C:\long\path", true, "file:///C:/long/path"),
            (r"\\?\UNC\server\share\f", true, "file://server/share/f"),
        ] {
            assert_eq!(file_uri(p.as_bytes(), windows), want, "{p}");
        }
    }

    #[test]
    fn punycode_matches_rfc_3492() {
        for (u, a) in [("münchen", "mnchen-3ya"), ("bücher", "bcher-kva"), ("例え", "r8jz45g"), ("faß", "fa-hia")]
        {
            assert_eq!(punycode(u).as_deref(), Some(a), "{u}");
        }
    }
}

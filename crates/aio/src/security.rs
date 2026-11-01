//! The security headers every answer carries (`[server.security]`, aio
//! `security`), decided once at boot from what the app already said (all
//! pure). A default is on only when it cannot break a working app:
//!
//! - **Framing**: `frame-ancestors` comes from `[server] hosts`, the same list
//!   that decides whose Origin may open the socket, so a page that may embed
//!   the app is exactly one that could use it. Nobody named: `'self'` and
//!   `X-Frame-Options: SAMEORIGIN` (it can't say a list, so then it is left out).
//! - **CSP** `basic`: `base-uri`, `object-src`, `form-action` and a
//!   `script-src` naming every source a page could use but `'unsafe-eval'`;
//!   no `default-src`, so off-origin assets still load. `strict` adds
//!   `default-src 'self'` and per-type sources; with `nonce`, the shell's
//!   scripts carry a per-answer nonce instead of `'unsafe-inline'`.
//!   `directives` replace (`"img-src" = "'self' https:"`) or drop
//!   (`"base-uri" = false`) one; any other `csp` is a policy used verbatim.
//! - **HSTS** only behind the operator's own certificate (`[server] cert`):
//!   pinning https for a name on aio's local root would outlive the app.
//! - No `Permissions-Policy` is guessed (camera, mic: the app says).
//!
//! A value no header can carry is refused at boot, naming the key, never a
//! 500 on every answer.
pub use aio_config::security::{Directive, Hsts, Security, frame_ancestors, headers, host_source, policy};

/// What a server sends: the headers every answer gets (unless it set its
/// own), and the shell's per-answer nonce policy.
#[derive(Clone, Debug)]
pub struct Page {
    pub headers: Vec<(&'static str, String)>,
    nonced: Option<(Security, String)>,
}

impl Default for Page {
    fn default() -> Self {
        Self::new(&Security::default(), &[], false).expect("the defaults are valid headers")
    }
}

impl Page {
    pub fn new(s: &Security, hosts: &[String], own_cert: bool) -> Result<Self, String> {
        let nonced = (s.headers && s.nonce && s.csp == "strict").then(|| (s.clone(), frame_ancestors(hosts)));
        Ok(Self { headers: headers(s, hosts, own_cert)?, nonced })
    }

    /// A shell page as sent: every script stamped with a fresh nonce, and
    /// the policy naming it; `None` when no nonce is asked for.
    pub fn stamp(&self, html: &str) -> Option<(String, String)> {
        let (s, ancestors) = self.nonced.as_ref()?;
        let n = nonce();
        let csp = policy(s, ancestors, Some(&n)).ok().flatten()?;
        Some((with_nonce(html, &n), csp))
    }
}

/// 128 random bits, base64url: nothing a header or attribute must escape.
pub fn nonce() -> String {
    use base64::Engine;
    let mut b = [0u8; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).expect("the OS random source");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// `html` with `nonce` on every `<script` that has none: one missed is a
/// blank page under a nonce policy.
pub fn with_nonce(html: &str, nonce: &str) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let mut rest = html;
    while let Some(i) = rest.find("<script") {
        let after = &rest[i + 7..];
        out.push_str(&rest[..i + 7]);
        let tag = &after[..after.find('>').unwrap_or(after.len())];
        if after.starts_with(['>', ' ', '\t', '\n', '\r', '/']) && !tag.contains(" nonce=") {
            out.push_str(&format!(" nonce=\"{nonce}\""));
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(h: &'a [(&str, String)], name: &str) -> Option<&'a str> {
        h.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
    }

    fn csp(s: &Security, nonce: Option<&str>) -> String {
        policy(s, "'self'", nonce).unwrap().unwrap()
    }

    fn with(f: impl FnOnce(&mut Security)) -> Security {
        let mut s = Security::default();
        f(&mut s);
        s
    }

    #[test]
    fn the_defaults_cannot_break_a_page() {
        let h = headers(&Security::default(), &[], false).unwrap();
        assert_eq!(get(&h, "x-content-type-options"), Some("nosniff"));
        assert_eq!(get(&h, "referrer-policy"), Some("no-referrer"));
        assert_eq!(get(&h, "x-frame-options"), Some("SAMEORIGIN"));
        assert_eq!(
            get(&h, "content-security-policy"),
            Some(
                "base-uri 'self'; object-src 'none'; frame-ancestors 'self'; form-action 'self'; script-src * data: \
                 blob: 'unsafe-inline' 'wasm-unsafe-eval'"
            )
        );
        // No default-src or per-type source: off-origin assets still load;
        // script-src withholds only 'unsafe-eval'.
        let c = csp(&Security::default(), None);
        for off in ["default-src", "style-src", "img-src", "connect-src", "font-src", "'unsafe-eval'"] {
            assert!(!c.contains(off), "{off}");
        }
        assert!(get(&h, "strict-transport-security").is_none() && get(&h, "permissions-policy").is_none());
    }

    #[test]
    fn framing_follows_the_hosts_that_may_open_the_socket() {
        let hs = |h: &[&str]| frame_ancestors(&h.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(hs(&[]), "'self'");
        assert_eq!(hs(&["dash.corp", "dash.corp"]), "'self' dash.corp:*");
        assert_eq!(hs(&["https://dash.corp/", "shell.internal:8080"]), "'self' https://dash.corp shell.internal:8080");
        assert_eq!(hs(&["*"]), "*");
        // Named embedders: no X-Frame-Options, which can't say a list.
        let h = headers(&Security::default(), &["dash.corp".into()], false).unwrap();
        assert!(get(&h, "x-frame-options").is_none());
        assert!(get(&h, "content-security-policy").unwrap().contains("frame-ancestors 'self' dash.corp:*"));
        assert!(get(&headers(&with(|s| s.frame_options = false), &[], false).unwrap(), "x-frame-options").is_none());
    }

    #[test]
    fn a_host_that_cannot_be_a_source_is_dropped_not_spliced() {
        for bad in
            ["ünïcödé.example", "\u{200b}", "a.com, b.com", "a.com; script-src *", "has space.com", "a.com\nb.com", ""]
        {
            assert_eq!(host_source(bad), None, "{bad:?}");
            assert_eq!(frame_ancestors(&[bad.into()]), "'self'");
        }
        for (good, want) in [
            ("https://dash.corp/path", "https://dash.corp"),
            ("http://a.com:8080", "http://a.com:8080"),
            ("*.corp.example", "*.corp.example:*"),
            ("127.0.0.1:3000", "127.0.0.1:3000"),
            ("[::1]:3000", "[::1]:3000"),
            ("[::1]", "[::1]:*"),
            ("a.com:*", "a.com:*"),
        ] {
            assert_eq!(host_source(good).as_deref(), Some(want), "{good}");
        }
    }

    #[test]
    fn hsts_only_behind_the_operators_certificate() {
        let hsts =
            |s: &Security, own| get(&headers(s, &[], own).unwrap(), "strict-transport-security").map(String::from);
        assert_eq!(hsts(&Security::default(), false), None);
        assert_eq!(hsts(&Security::default(), true).as_deref(), Some("max-age=15552000"));
        let long = with(|s| s.hsts = Hsts::Value("max-age=63072000; includeSubDomains".into()));
        assert_eq!(hsts(&long, true).as_deref(), Some("max-age=63072000; includeSubDomains"));
        assert_eq!(hsts(&with(|s| s.hsts = Hsts::On(false)), true), None);
    }

    #[test]
    fn every_piece_is_switchable_and_the_whole_policy_is_spelled_out() {
        let only = headers(&with(|s| s.headers = false), &[], true).unwrap();
        assert_eq!(only, [("x-content-type-options", "nosniff".to_string())]);
        assert_eq!(policy(&with(|s| s.csp = "off".into()), "'self'", None), Ok(None));
        assert_eq!(csp(&with(|s| s.csp = "default-src 'none'".into()), None), "default-src 'none'");
        let r = with(|s| s.referrer = "strict-origin-when-cross-origin".into());
        assert_eq!(get(&headers(&r, &[], false).unwrap(), "referrer-policy"), Some("strict-origin-when-cross-origin"));
        let p = with(|s| s.permissions = Some("camera=()".into()));
        assert_eq!(get(&headers(&p, &[], false).unwrap(), "permissions-policy"), Some("camera=()"));
        let strict = with(|s| s.csp = "strict".into());
        assert_eq!(
            headers(&strict, &["a.com".into()], true).unwrap(),
            [
                ("x-content-type-options", "nosniff".to_string()),
                ("referrer-policy", "no-referrer".into()),
                (
                    "content-security-policy",
                    "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'self' a.com:*; \
                     form-action 'self'; script-src 'self' blob: 'wasm-unsafe-eval' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; \
                     img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self' ws: wss:"
                        .into()
                ),
                ("strict-transport-security", "max-age=15552000".into()),
            ]
        );
    }

    #[test]
    fn directives_drop_widen_and_compose_with_the_nonce() {
        let d = |kv: &[(&str, Directive)]| {
            with(|s| s.directives = kv.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
        };
        let c = csp(&d(&[("base-uri", Directive::Off(false))]), None);
        assert!(!c.contains("base-uri") && c.contains("object-src 'none'"));
        let c = csp(&d(&[("img-src", Directive::Set("'self' https:".into()))]), None);
        assert!(c.ends_with("; img-src 'self' https:"), "{c}");
        // Ignored for a verbatim policy; all dropped: no header at all.
        let mut v = d(&[("base-uri", Directive::Off(false))]);
        v.csp = "default-src 'none'".into();
        assert_eq!(csp(&v, None), "default-src 'none'");
        let all = ["base-uri", "object-src", "frame-ancestors", "form-action", "script-src"];
        let none = d(&all.map(|k| (k, Directive::Off(false))));
        assert!(get(&headers(&none, &[], false).unwrap(), "content-security-policy").is_none());
        // A nonce replaces 'unsafe-inline' for scripts only; strict without one keeps it.
        let mut s = d(&[]);
        (s.csp, s.nonce) = ("strict".into(), true);
        let c = csp(&s, Some("n0"));
        assert!(c.contains("script-src 'self' blob: 'wasm-unsafe-eval' 'nonce-n0'") && c.contains("style-src 'self' 'unsafe-inline'"), "{c}");
        assert!(csp(&with(|s| s.csp = "strict".into()), None).contains("script-src 'self' blob: 'wasm-unsafe-eval' 'unsafe-inline'"));
        // An app's script host and the nonce, not one or the other.
        let mut s = d(&[("script-src", Directive::Set("'self' https://cdn.example".into()))]);
        (s.csp, s.nonce) = ("strict".into(), true);
        assert!(csp(&s, Some("n0")).contains("script-src 'self' https://cdn.example 'nonce-n0'"));
        assert!(
            csp(&d(&[("script-src", Directive::Set("'self' x.com".into()))]), None)
                .ends_with("script-src 'self' x.com")
        );
        let mut s = d(&[("style-src", Directive::Set("'self' {nonce}".into()))]);
        s.nonce = true;
        assert!(csp(&s, Some("n0")).contains("style-src 'self' 'nonce-n0'"));
        s.nonce = false;
        assert!(policy(&s, "'self'", None).unwrap_err().contains("nonce is off"));
        for bad in [
            ("bad name", Directive::Off(false)),
            ("img-src", Directive::Off(true)),
            ("img-src", Directive::Set("a; b".into())),
        ] {
            assert!(policy(&d(std::slice::from_ref(&bad)), "'self'", None).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_value_no_header_can_carry_is_refused_at_boot() {
        for s in [
            with(|s| s.permissions = Some("camera=(ünï)".into())),
            with(|s| s.permissions = Some("camera=()\r\nX-Injected: yes".into())),
            with(|s| s.referrer = "no-referrer\n".into()),
        ] {
            assert!(headers(&s, &[], false).unwrap_err().contains("can't go in an HTTP header"));
        }
    }

    #[test]
    fn every_shell_script_carries_a_fresh_nonce() {
        let html = "<html><script>a()</script><script type=module src=/app.js></script><script nonce=\"x\">b</script><scripts></html>";
        let out = with_nonce(html, "N");
        assert_eq!(out.matches("nonce=\"N\"").count(), 2, "{out}");
        assert!(out.contains("<script nonce=\"x\">") && out.contains("<scripts>"));
        let (a, b) = (nonce(), nonce());
        assert!(a != b && a.len() == 22 && a.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
        let mut s = with(|s| s.csp = "strict".into());
        assert!(Page::new(&s, &[], false).unwrap().stamp(html).is_none(), "no nonce asked: the shell as it is");
        s.nonce = true;
        let page = Page::new(&s, &[], false).unwrap();
        let (sent, csp) = page.stamp(html).unwrap();
        let n = csp.split("'nonce-").nth(1).unwrap().split('\'').next().unwrap();
        assert!(sent.contains(&format!("<script nonce=\"{n}\">a()")), "{sent}");
        assert_ne!(page.stamp(html).unwrap().1, csp, "per answer");
        // Everything else gets the strict policy without a nonce.
        assert!(get(&page.headers, "content-security-policy").unwrap().contains("script-src 'self' blob: 'wasm-unsafe-eval' 'unsafe-inline'"));
    }
}

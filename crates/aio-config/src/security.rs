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
//!   `default-src 'self'` and per-type sources (scripts: its own, those a page
//!   builds (`blob:`) and wasm, as aio's web page needs); with `nonce`, the shell's
//!   scripts carry a per-answer nonce instead of `'unsafe-inline'`.
//!   `directives` replace (`"img-src" = "'self' https:"`) or drop
//!   (`"base-uri" = false`) one; any other `csp` is a policy used verbatim.
//! - **HSTS** only behind the operator's own certificate (`[server] cert`):
//!   pinning https for a name on aio's local root would outlive the app.
//! - No `Permissions-Policy` is guessed (camera, mic: the app says).
//!
//! A value no header can carry is refused at boot, naming the key, never a
//! 500 on every answer.
use aio_macros::Lit;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `[server.security]`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(default, deny_unknown_fields)]
pub struct Security {
    /// Send them (`false`: `nosniff` only).
    pub headers: bool,
    /// `basic`, `strict`, `off`, or a policy used verbatim.
    pub csp: String,
    /// Per directive: a value replaces or adds it, `false` drops it (not for
    /// a verbatim policy). `{nonce}` names the nonce.
    pub directives: BTreeMap<String, Directive>,
    /// `X-Frame-Options: SAMEORIGIN` when no host may embed the app.
    pub frame_options: bool,
    pub referrer: String,
    /// `Strict-Transport-Security` behind an operator's certificate: `true`
    /// is 180 days, a string is used verbatim.
    pub hsts: Hsts,
    /// `Permissions-Policy`, verbatim.
    pub permissions: Option<String>,
    /// A per-answer nonce on the shell's scripts (`strict` only).
    pub nonce: bool,
}

impl Default for Security {
    fn default() -> Self {
        Self {
            headers: true,
            csp: "basic".into(),
            directives: BTreeMap::new(),
            frame_options: true,
            referrer: "no-referrer".into(),
            hsts: Hsts::On(true),
            permissions: None,
            nonce: false,
        }
    }
}

/// One directive's override.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(untagged)]
pub enum Directive {
    /// `false`: drop it.
    Off(bool),
    Set(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Lit)]
#[serde(untagged)]
pub enum Hsts {
    On(bool),
    Value(String),
}

const BASIC_SCRIPTS: &str = "* data: blob: 'unsafe-inline' 'wasm-unsafe-eval'";

/// The `frame-ancestors` sources: `'self'` and each host that can be one
/// (any port, as the Origin check takes it); `*` lets anyone.
pub fn frame_ancestors(hosts: &[String]) -> String {
    let mut out = vec!["'self'".to_string()];
    for h in hosts.iter().map(|h| h.trim()) {
        if h == "*" {
            return "*".into();
        }
        if let Some(s) = host_source(h)
            && !out.contains(&s)
        {
            out.push(s);
        }
    }
    out.join(" ")
}

/// `entry` as a CSP host source, or `None` when it can't be one (a comma,
/// a `;`, non-ASCII: spliced in, it would break or widen the policy). A
/// bare name gets `:*`, a URL keeps its scheme and port.
pub fn host_source(entry: &str) -> Option<String> {
    let (scheme, rest) = match entry.split_once("://") {
        Some((s, r)) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b)) => {
            (format!("{}://", s.to_ascii_lowercase()), r)
        }
        Some(_) => return None,
        None => (String::new(), entry),
    };
    let hp = rest.split('/').next().unwrap_or_default();
    let (host, port) = match hp.rfind(':') {
        Some(i) if !hp.ends_with(']') => (&hp[..i], Some(&hp[i + 1..])),
        _ => (hp, None),
    };
    let label = |l: &str| {
        !l.is_empty()
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    let name = host.strip_prefix("*.").unwrap_or(host);
    let ip6 = host.len() > 2
        && host.starts_with('[')
        && host.ends_with(']')
        && host[1..host.len() - 1].bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.');
    let port_ok =
        port.is_none_or(|p| p == "*" || (!p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit())));
    if !(ip6 || (!name.is_empty() && name.split('.').all(label))) || !port_ok {
        return None;
    }
    let port = match (port, scheme.is_empty()) {
        (Some(p), _) => format!(":{p}"),
        (None, true) => ":*".into(),
        (None, false) => String::new(),
    };
    Some(format!("{scheme}{host}{port}"))
}

/// The Content-Security-Policy, or `None` for none. `nonce`: the one this
/// answer's scripts carry.
pub fn policy(s: &Security, ancestors: &str, nonce: Option<&str>) -> Result<Option<String>, String> {
    let strict = match s.csp.as_str() {
        "off" => return Ok(None),
        "basic" => false,
        "strict" => true,
        verbatim => return Ok(Some(verbatim.to_string())),
    };
    let mut d: Vec<(String, String)> = [
        ("base-uri", "'self'"),
        ("object-src", "'none'"),
        ("frame-ancestors", ancestors),
        ("form-action", "'self'"),
        ("script-src", BASIC_SCRIPTS),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    if strict {
        d.insert(0, ("default-src".into(), "'self'".into()));
        let scripts = match nonce {
            Some(n) => format!("'self' blob: 'wasm-unsafe-eval' 'nonce-{n}'"),
            None => "'self' blob: 'wasm-unsafe-eval' 'unsafe-inline'".into(),
        };
        set(&mut d, "script-src", scripts);
        // Styles keep 'unsafe-inline': it also governs `style=` attributes.
        for (k, v) in [
            ("style-src", "'self' 'unsafe-inline'"),
            ("img-src", "'self' data: blob:"),
            ("font-src", "'self' data:"),
            ("connect-src", "'self' ws: wss:"),
        ] {
            set(&mut d, k, v.into());
        }
    }
    for (name, v) in &s.directives {
        let key = name.trim().to_ascii_lowercase();
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            return Err(format!("[server.security] directives: {name:?} is not a directive name"));
        }
        match v {
            Directive::Off(false) => d.retain(|(k, _)| *k != key),
            Directive::Off(true) => {
                return Err(format!("[server.security] directives.{key}: a value, or false to drop it"));
            }
            Directive::Set(v) => {
                let v = v.trim();
                if v.contains([';', ',']) {
                    return Err(format!("[server.security] directives.{key}: {v:?} holds a ; or , (one directive)"));
                }
                // The nonce composes with the app's sources, never lost to them.
                // Off the shell, no answer has a nonce: `{nonce}` names none.
                let named = nonce.map(|n| format!("'nonce-{n}'")).unwrap_or_default();
                let v = match nonce {
                    _ if v.contains("{nonce}") && !s.nonce => {
                        return Err(format!(
                            "[server.security] directives.{key} uses {{nonce}}, but nonce is off: set nonce = true"
                        ));
                    }
                    _ if v.contains("{nonce}") => v.replace("{nonce}", &named).trim().to_string(),
                    Some(_) if key == "script-src" && !v.contains("'nonce-") => format!("{v} {named}"),
                    _ => v.to_string(),
                };
                set(&mut d, &key, v);
            }
        }
    }
    Ok((!d.is_empty()).then(|| d.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join("; ")))
}

fn set(d: &mut Vec<(String, String)>, key: &str, v: String) {
    match d.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = v,
        None => d.push((key.into(), v)),
    }
}

/// The headers, as `(name, value)`. `own_cert`: served over TLS with the
/// operator's certificate. A value no header can carry is an error.
pub fn headers(s: &Security, hosts: &[String], own_cert: bool) -> Result<Vec<(&'static str, String)>, String> {
    let mut out = vec![("x-content-type-options", "nosniff".to_string())];
    if !s.headers {
        return Ok(out);
    }
    out.push(("referrer-policy", s.referrer.clone()));
    let ancestors = frame_ancestors(hosts);
    if ancestors == "'self'" && s.frame_options {
        out.push(("x-frame-options", "SAMEORIGIN".into()));
    }
    // A nonce is per answer: only the shell's own policy carries one ([`Page`]).
    if let Some(csp) = policy(s, &ancestors, None)? {
        out.push(("content-security-policy", csp));
    }
    match (&s.hsts, own_cert) {
        (Hsts::On(true), true) => out.push(("strict-transport-security", "max-age=15552000".into())),
        (Hsts::Value(v), true) => out.push(("strict-transport-security", v.clone())),
        _ => {}
    }
    if let Some(p) = &s.permissions {
        out.push(("permissions-policy", p.clone()));
    }
    for (name, v) in &out {
        if let Some(bad) = v.chars().find(|c| !(' '..='~').contains(c)) {
            return Err(format!(
                "[server.security]: the {name} value holds {bad:?}, which can't go in an HTTP header (every answer \
                 would fail): {v:?}"
            ));
        }
    }
    Ok(out)
}

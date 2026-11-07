//! The HTTP gate in front of every route, decided once, as aio does:
//!
//! - **Host**: a request must be addressed to this app: an IP literal,
//!   `localhost`, a `.local` name, or a name the app lists. Any other name is
//!   the shape of DNS rebinding: a page on `evil.example` whose DNS flips to
//!   this machine would otherwise be same-origin with the app.
//! - **Origin**: a browser says where a request comes from. Absent (native
//!   clients, tools) is fine; present, it must be this same host or a listed one.
//!   With `strict_origin`, a socket upgrade to an app open to the network must
//!   say it (an origin-stripping proxy or sandbox would skip the check).
//! - **POST** bodies must be JSON: a cross-site form can't send that without a
//!   CORS preflight, which nothing here answers (CSRF). One exception: a
//!   browser navigating by a form POST ([`navigation`]: a payment's return
//!   page, a `form_post`) to a client route gets the shell, and nothing runs.
//! - **Proxy**: behind a reverse proxy every client is the proxy's address.
//!   With `trust_proxy` (the header the proxy overwrites, e.g.
//!   `x-forwarded-for`) the client is that header's LAST hop, the one the
//!   proxy wrote (the first is whatever the client sent), and the proxy's
//!   `X-Forwarded-Proto: https` makes cookies `Secure`. A relayed request is
//!   never a same-machine one.
//!
//! Every answer also carries the security headers ([`crate::security`]).
use axum::http::HeaderMap;
use std::net::IpAddr;

/// Why a request is refused (status, said to the sender), or `None`.
/// `need_origin`: a socket upgrade under `strict_origin` on an exposed app.
pub fn refusal(
    host: Option<&str>,
    origin: Option<&str>,
    post: bool,
    content_type: Option<&str>,
    hosts: &[String],
    need_origin: bool,
) -> Option<(u16, String)> {
    let Some(host) = host.filter(|h| allowed(name_of(h), hosts)) else {
        let h = host.unwrap_or("").chars().take(100).collect::<String>();
        return Some((
            403,
            format!(
                "Host \"{h}\" is not a name this app is served as (an IP address, localhost, a .local name, this \
                 machine's name, or one listed in [server] hosts)"
            ),
        ));
    };
    if need_origin && origin.is_none_or(str::is_empty) {
        return Some((403, "this app takes sockets only from pages that say their Origin (strict_origin)".into()));
    }
    if let Some(o) = origin {
        let from = o.split_once("://").map(|(_, rest)| rest.trim_end_matches('/'));
        let ok = from
            .is_some_and(|f| f.eq_ignore_ascii_case(host) || hosts.iter().any(|n| name_of(f).eq_ignore_ascii_case(n)));
        if !ok {
            let o = o.chars().take(100).collect::<String>();
            return Some((403, format!("requests from origin \"{o}\" are not allowed")));
        }
    }
    let json = content_type
        .is_some_and(|c| c.split(';').next().is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json")));
    if post && !json {
        return Some((415, "send the body as Content-Type: application/json".into()));
    }
    None
}

/// A browser navigating to a page by a form POST: its fetch metadata says
/// so, or (none: plain-http LAN pages, older Safari) it asks for HTML.
pub fn navigation(headers: &HeaderMap) -> bool {
    let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok());
    match h("sec-fetch-mode") {
        Some(mode) => mode == "navigate" && h("sec-fetch-dest") == Some("document"),
        None => h("accept").is_some_and(|a| a.contains("text/html")),
    }
}

/// Headers a proxy adds to say "the real client is someone else".
const FORWARDED: [&str; 5] = ["x-forwarded-for", "forwarded", "x-real-ip", "cf-connecting-ip", "true-client-ip"];

/// Whether a proxy relayed this request (the `trust`ed header or a usual one).
pub fn relayed(headers: &HeaderMap, trust: Option<&str>) -> bool {
    trust.into_iter().chain(FORWARDED).any(|h| headers.contains_key(h))
}

/// The client: `trust`'s last hop (an address, with or without a port), else
/// the TCP `peer`.
pub fn client(peer: IpAddr, headers: &HeaderMap, trust: Option<&str>) -> IpAddr {
    last_hop(headers, trust).unwrap_or(peer)
}

fn last_hop(headers: &HeaderMap, trust: Option<&str>) -> Option<IpAddr> {
    let hops = headers.get_all(trust?).iter().filter_map(|v| v.to_str().ok()).next_back()?;
    let hop = hops.rsplit(',').next()?.trim();
    let bare = hop.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(hop);
    bare.parse().ok().or_else(|| hop.parse::<std::net::SocketAddr>().ok().map(|a| a.ip()))
}

/// Whether the browser is on https: behind a `trust`ed proxy that says so.
pub fn proxied_https(headers: &HeaderMap, trust: Option<&str>) -> bool {
    trust.is_some()
        && headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .is_some_and(|p| p.trim().eq_ignore_ascii_case("https"))
}

/// What to warn about the proxy setup, from a real request: forwarding
/// headers with no `trust` (every client shares the proxy's budgets), or a
/// `trust` no proxy wrote (the caller picks its own address).
pub fn proxy_warning(peer: IpAddr, headers: &HeaderMap, trust: Option<&str>) -> Option<String> {
    match trust {
        None => FORWARDED.into_iter().find(|h| headers.contains_key(*h)).map(|h| {
            format!(
                "a request carried \"{h}\" but [server] trust_proxy is not set: behind a proxy every client shares \
                 one address for the per-IP budgets (sign-in, pairing). Set trust_proxy = \"{h}\" if the proxy \
                 overwrites it"
            )
        }),
        Some(t) => match last_hop(headers, trust) {
            Some(hop) if hop != peer => None,
            _ => Some(format!(
                "[server] trust_proxy = \"{t}\" but a request came without a proxy's hop in it: the caller then \
                 picks its own address for the per-IP budgets. Remove trust_proxy unless a proxy overwrites \"{t}\""
            )),
        },
    }
}

/// This machine's own name, lowercase, once (`None`: the OS gave none): a
/// LAN browser reaches an exposed app by it, so it is served as the app
/// (aio's machine hostname).
pub fn machine_name() -> Option<String> {
    static NAME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        #[cfg(unix)]
        let name = {
            let mut b = [0u8; 256];
            // SAFETY: the buffer and its length are this call's own.
            let ok = unsafe { libc::gethostname(b.as_mut_ptr().cast(), b.len()) } == 0;
            ok.then(|| String::from_utf8_lossy(&b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]).into_owned())
        };
        #[cfg(windows)]
        let name = std::env::var("COMPUTERNAME").ok();
        #[cfg(not(any(unix, windows)))]
        let name = None;
        name.map(|n| n.trim().to_ascii_lowercase()).filter(|n| !n.is_empty())
    })
    .clone()
}

/// `host[:port]` → host (an IPv6 literal keeps its brackets).
pub(crate) fn name_of(h: &str) -> &str {
    match h.rfind(']') {
        Some(end) if h.starts_with('[') => &h[..=end],
        _ => h.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(h, |(n, _)| n),
    }
}

fn allowed(name: &str, hosts: &[String]) -> bool {
    let n = name.trim_end_matches('.').to_ascii_lowercase();
    let ip = n.strip_prefix('[').and_then(|v| v.strip_suffix(']')).unwrap_or(&n);
    !n.is_empty()
        && (ip.parse::<IpAddr>().is_ok()
            || n == "localhost"
            || n.ends_with(".localhost")
            || n.ends_with(".local")
            || hosts.iter().any(|h| h.eq_ignore_ascii_case(&n)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(host: &str, origin: Option<&str>) -> Option<u16> {
        refusal(Some(host), origin, false, None, &["notes.example.org".into()], false).map(|r| r.0)
    }

    fn headers(kv: &[(&str, &str)]) -> HeaderMap {
        kv.iter().map(|(k, v)| (k.parse().unwrap(), v.parse().unwrap())).collect()
    }

    const LOOPBACK: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

    #[test]
    fn only_this_apps_names_are_served() {
        for ok in ["127.0.0.1:8090", "[::1]:8090", "192.168.1.5", "localhost:1", "box.local", "notes.example.org"] {
            assert_eq!(get(ok, None), None, "{ok}");
        }
        for bad in ["evil.example:8090", "", "localhost.evil.example", "[::1"] {
            assert_eq!(get(bad, None), Some(403), "{bad}");
        }
        assert_eq!(refusal(None, None, false, None, &[], false).map(|r| r.0), Some(403));
    }

    #[test]
    fn a_browser_origin_must_be_this_same_app() {
        assert_eq!(get("127.0.0.1:8090", Some("http://127.0.0.1:8090")), None);
        assert_eq!(get("box.local:8090", Some("https://notes.example.org")), None);
        for bad in ["http://evil.example", "http://127.0.0.1:9999", "null", "file://"] {
            assert_eq!(get("127.0.0.1:8090", Some(bad)), Some(403), "{bad}");
        }
    }

    #[test]
    fn a_form_post_navigation_says_what_it_is() {
        let h = |kv: &[(&'static str, &'static str)]| {
            let mut m = HeaderMap::new();
            for (k, v) in kv {
                m.insert(*k, v.parse().unwrap());
            }
            navigation(&m)
        };
        assert!(h(&[("sec-fetch-mode", "navigate"), ("sec-fetch-dest", "document")]));
        assert!(h(&[("accept", "text/html,application/xhtml+xml,*/*;q=0.8")]));
        assert!(!h(&[("sec-fetch-mode", "cors"), ("sec-fetch-dest", "empty"), ("accept", "text/html")]));
        assert!(!h(&[("sec-fetch-mode", "navigate"), ("sec-fetch-dest", "iframe")]));
        assert!(!h(&[("accept", "*/*")]));
        assert!(!h(&[]));
    }

    #[test]
    fn strict_origin_needs_a_said_origin() {
        let ws = |o| refusal(Some("192.168.1.5:8090"), o, false, None, &[], true).map(|r| r.0);
        assert_eq!(ws(None), Some(403));
        assert_eq!(ws(Some("")), Some(403));
        assert_eq!(ws(Some("http://192.168.1.5:8090")), None);
        assert_eq!(ws(Some("http://evil.example")), Some(403));
    }

    #[test]
    fn posts_must_be_json() {
        let post = |ct| refusal(Some("127.0.0.1"), None, true, ct, &[], false).map(|r| r.0);
        assert_eq!(post(Some("application/json")), None);
        assert_eq!(post(Some("Application/JSON; charset=utf-8")), None);
        assert_eq!(post(Some("text/plain")), Some(415));
        assert_eq!(post(Some("application/x-www-form-urlencoded")), Some(415));
        assert_eq!(post(None), Some(415));
    }

    #[test]
    fn the_client_is_the_trusted_last_hop() {
        let xff = headers(&[("x-forwarded-for", "198.51.100.9, 203.0.113.7")]);
        assert_eq!(client(LOOPBACK, &xff, Some("x-forwarded-for")).to_string(), "203.0.113.7");
        // Not trusted: the peer, whatever the client says.
        assert_eq!(client(LOOPBACK, &xff, None), LOOPBACK);
        assert_eq!(client(LOOPBACK, &xff, Some("x-real-ip")), LOOPBACK);
        for (hop, ip) in
            [("[2001:db8::1]:443", "2001:db8::1"), ("2001:db8::2", "2001:db8::2"), ("10.0.0.3:5000", "10.0.0.3")]
        {
            assert_eq!(client(LOOPBACK, &headers(&[("x-real-ip", hop)]), Some("x-real-ip")).to_string(), ip, "{hop}");
        }
        assert_eq!(client(LOOPBACK, &headers(&[("x-real-ip", "unknown")]), Some("x-real-ip")), LOOPBACK);
    }

    #[test]
    fn a_relayed_request_is_never_local_and_https_comes_from_a_trusted_proxy() {
        assert!(!relayed(&headers(&[]), None));
        assert!(relayed(&headers(&[("forwarded", "for=192.0.2.1")]), None));
        assert!(relayed(&headers(&[("x-client", "192.0.2.1")]), Some("x-client")));
        let https = headers(&[("x-forwarded-proto", "HTTPS, http")]);
        assert!(proxied_https(&https, Some("x-forwarded-for")));
        assert!(!proxied_https(&https, None));
        assert!(!proxied_https(&headers(&[("x-forwarded-proto", "http")]), Some("x-forwarded-for")));
    }

    #[test]
    fn proxy_setups_that_defeat_the_budgets_are_said() {
        let xff = headers(&[("x-forwarded-for", "203.0.113.7")]);
        assert!(proxy_warning(LOOPBACK, &xff, None).unwrap().contains("trust_proxy = \"x-forwarded-for\""));
        assert_eq!(proxy_warning(LOOPBACK, &headers(&[]), None), None);
        assert_eq!(proxy_warning(LOOPBACK, &xff, Some("x-forwarded-for")), None);
        assert!(proxy_warning(LOOPBACK, &headers(&[]), Some("x-forwarded-for")).is_some());
        let own = headers(&[("x-forwarded-for", "127.0.0.1")]);
        assert!(proxy_warning(LOOPBACK, &own, Some("x-forwarded-for")).is_some());
    }
}

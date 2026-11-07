//! LAN discovery, aio's UDP format (`server/discovery.ts`): a device sends
//! `AIO_DISCOVER? v1 [nonce]`; each app answers `AIO1 {json}`. Nothing is
//! announced on a timer, only private or loopback senders get answers, and at
//! most 5 a second each.
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

pub const PORT: u16 = 8099;
pub const WAIT: Duration = Duration::from_millis(1200);
const PER_SECOND: usize = 5;

/// What an app tells the LAN about itself (aio's reply fields).
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub name: String,
    pub port: u16,
    pub title: Option<String>,
    pub needs_auth: bool,
    pub tls: bool,
    /// Where the answer came from (set by `probe`).
    pub host: Option<IpAddr>,
}

/// The port from `AIO_DISCOVERY_PORT`, else 8099.
pub fn port() -> u16 {
    std::env::var("AIO_DISCOVERY_PORT").ok().and_then(|p| p.parse().ok()).filter(|p| *p > 0).unwrap_or(PORT)
}

fn good_nonce(n: &str) -> bool {
    (1..=64).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The nonce of a valid probe (`Some(None)` for a probe without one).
fn parse_probe(msg: &[u8]) -> Option<Option<&str>> {
    let rest = std::str::from_utf8(msg).ok()?.trim_end().strip_prefix("AIO_DISCOVER? v1")?;
    match rest.strip_prefix(' ') {
        None if rest.is_empty() => Some(None),
        Some(n) if good_nonce(n) => Some(Some(n)),
        _ => None,
    }
}

/// The answer to a probe; `addrs`: this machine's own IPv4 addresses, so a
/// prober that heard it from two of them lists it once (aio's).
fn reply(app: &Found, nonce: Option<&str>, addrs: &[Ipv4Addr]) -> Vec<u8> {
    let mut v = json!({ "name": app.name, "port": app.port, "needsAuth": app.needs_auth, "tls": app.tls });
    if !addrs.is_empty() {
        v["addrs"] = json!(addrs);
    }
    if let Some(t) = &app.title {
        v["title"] = json!(t);
    }
    if let Some(n) = nonce {
        v["nonce"] = json!(n);
    }
    format!("AIO1 {v}").into_bytes()
}

/// An answer: the app, its nonce, and the answering machine's addresses.
type Heard = (Found, Option<String>, Vec<IpAddr>);

fn parse_reply(msg: &[u8], host: IpAddr) -> Option<Heard> {
    let v: Value = serde_json::from_slice(msg.strip_prefix(b"AIO1 ")?).ok()?;
    let found = Found {
        name: v["name"].as_str()?.to_string(),
        port: u16::try_from(v["port"].as_u64()?).ok().filter(|p| *p > 0)?,
        title: v["title"].as_str().map(str::to_string),
        needs_auth: v["needsAuth"].as_bool().unwrap_or(true),
        tls: v["tls"].as_bool().unwrap_or(false),
        host: Some(host),
    };
    let addrs = v["addrs"].as_array().map_or(vec![], |a| a.iter().filter_map(|x| x.as_str()?.parse().ok()).collect());
    Some((found, v["nonce"].as_str().map(str::to_string), addrs))
}

/// Where a probe goes (pure): the limited broadcast, and each IPv4
/// interface's own (`192.168.1.255` for `192.168.1.7/24`): the limited one
/// alone leaves by one interface (with a VPN up, usually the VPN's), and the
/// LAN is never asked.
pub fn targets(port: u16, interfaces: &[(Ipv4Addr, Ipv4Addr)]) -> Vec<SocketAddr> {
    let mut out = vec![SocketAddr::from((Ipv4Addr::BROADCAST, port))];
    for (addr, mask) in interfaces {
        let b = SocketAddr::from((Ipv4Addr::from(addr.to_bits() | !mask.to_bits()), port));
        if !addr.is_loopback() && *mask != Ipv4Addr::BROADCAST && !out.contains(&b) {
            out.push(b);
        }
    }
    out
}

/// Two answers are one machine heard from two of its addresses when each
/// one's source is among the other's own addresses (two machines on one
/// subnet share none; an aio older than 1.0.21 sends no list).
fn one_machine(a: (IpAddr, &[IpAddr]), b: (IpAddr, &[IpAddr])) -> bool {
    a.0 != b.0 && a.1.contains(&b.0) && b.1.contains(&a.0)
}

/// Why a probe asked nobody (pure): no target took it, so an empty answer
/// says nothing about what runs. On macOS the Local Network permission is
/// the usual cause, so it is named.
fn unsent(targets: &[SocketAddr], e: &std::io::Error, os: &str) -> String {
    let to: Vec<String> = targets.iter().map(ToString::to_string).collect();
    let mac = if os == "macos" {
        " On macOS this is the Local Network permission (System Settings → Privacy & Security → Local Network): \
         the app or terminal this runs under needs it, and a process that outlived its login session has none."
    } else {
        ""
    };
    format!("discovery: the probe could not be sent to {} ({e}): nobody was asked.{mac}", to.join(", "))
}

fn may_ask(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// A UDP socket on `port` that other apps on this machine (aio ones too) can
/// share: the options Node's `reuseAddr` sets.
fn shared_socket(port: u16) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    s.set_reuse_port(true)?;
    s.set_nonblocking(true)?;
    s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
    UdpSocket::from_std(s.into())
}

/// Answer probes on `port` with `apps`, until the socket fails.
pub async fn respond(port: u16, apps: Vec<Found>) -> std::io::Result<()> {
    let sock = shared_socket(port)?;
    let own: Vec<Ipv4Addr> = crate::tls::interfaces().into_iter().map(|(a, _)| a).filter(|a| !a.is_loopback()).collect();
    let mut seen: HashMap<IpAddr, Vec<Instant>> = HashMap::new();
    let mut buf = [0u8; 512];
    loop {
        let (n, from) = sock.recv_from(&mut buf).await?;
        let Some(nonce) = parse_probe(&buf[..n]) else {
            continue;
        };
        if !may_ask(from.ip()) {
            continue;
        }
        let now = Instant::now();
        seen.retain(|_, t| {
            t.retain(|at| now.duration_since(*at) < Duration::from_secs(1));
            !t.is_empty()
        });
        let times = seen.entry(from.ip()).or_default();
        for app in &apps {
            if times.len() >= PER_SECOND {
                break;
            }
            times.push(now);
            _ = sock.send_to(&reply(app, nonce, &own), from).await;
        }
    }
}

/// Ask `targets` and collect answers for `wait`; each app once (`host:port:name`,
/// this machine's addresses one host, and one machine heard from two of
/// its addresses once). An error when no target took the probe.
pub async fn probe(targets: &[SocketAddr], wait: Duration) -> std::io::Result<Vec<Found>> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    sock.set_broadcast(true)?;
    let nonce = crate::uuid_v4().simple().to_string();
    let msg = format!("AIO_DISCOVER? v1 {nonce}");
    let mut failed = None;
    let mut sent = false;
    for t in targets {
        match sock.send_to(msg.as_bytes(), t).await {
            Ok(_) => sent = true,
            Err(e) => _ = failed.get_or_insert(e),
        }
    }
    if let (false, Some(e)) = (sent, &failed) {
        return Err(std::io::Error::new(e.kind(), unsent(targets, e, std::env::consts::OS)));
    }
    let own: Vec<IpAddr> = crate::tls::local_addrs();
    let mut found: Vec<(Found, Vec<IpAddr>)> = Vec::new();
    let mut buf = [0u8; 2048];
    let end = tokio::time::Instant::now() + wait;
    while let Ok(Ok((n, from))) = tokio::time::timeout_at(end, sock.recv_from(&mut buf)).await {
        let Some((f, echo, addrs)) = parse_reply(&buf[..n], from.ip()) else {
            continue;
        };
        let here = |h: Option<IpAddr>| h.filter(|h| !own.contains(h));
        let seen = found.iter().any(|(g, a)| {
            (g.port, &g.name) == (f.port, &f.name)
                && (here(g.host) == here(f.host)
                    || g.host.zip(f.host).is_some_and(|(x, y)| one_machine((x, a), (y, &addrs))))
        });
        if echo.as_deref() == Some(&nonce) && !seen {
            found.push((f, addrs));
        }
    }
    Ok(found.into_iter().map(|(f, _)| f).collect())
}

/// Ask every network this machine is on ([`targets`]) on `port`, for `wait`.
pub async fn probe_lan(port: u16, wait: Duration) -> std::io::Result<Vec<Found>> {
    probe(&targets(port, &crate::tls::interfaces()), wait).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_and_replies_parse_like_aio() {
        assert_eq!(parse_probe(b"AIO_DISCOVER? v1"), Some(None));
        assert_eq!(parse_probe(b"AIO_DISCOVER? v1 ab_C-9"), Some(Some("ab_C-9")));
        assert_eq!(parse_probe(b"AIO_DISCOVER? v1 bad nonce"), None);
        assert_eq!(parse_probe(&[b"AIO_DISCOVER? v1 ".as_slice(), &[b'a'; 65]].concat()), None);
        assert_eq!(parse_probe(b"AIO_DISCOVER? v2"), None);
        let app = Found {
            name: "todo".into(),
            port: 8100,
            title: Some("Todo".into()),
            needs_auth: true,
            tls: false,
            host: None,
        };
        let host: IpAddr = "10.0.0.5".parse().unwrap();
        let own: Ipv4Addr = "10.0.0.5".parse().unwrap();
        let (back, nonce, addrs) = parse_reply(&reply(&app, Some("n1"), &[own]), host).unwrap();
        assert_eq!(back, Found { host: Some(host), ..app.clone() });
        assert_eq!((nonce.as_deref(), addrs), (Some("n1"), vec![host]));
        assert_eq!(parse_reply(&reply(&app, None, &[]), host).unwrap().2, Vec::<IpAddr>::new(), "an aio before 1.0.21: no list");
        assert!(parse_reply(br#"AIO1 {"name":"x","port":0}"#, host).is_none());
    }

    #[test]
    fn every_network_is_asked_and_one_machine_is_listed_once() {
        // aio tests/discovery broadcastTargets and oneMachine.
        let ip = |s: &str| s.parse::<Ipv4Addr>().unwrap();
        let ifs = [
            (ip("127.0.0.1"), ip("255.0.0.0")),
            (ip("192.168.1.7"), ip("255.255.255.0")),
            (ip("10.8.0.2"), ip("255.255.0.0")),
            (ip("192.168.1.9"), ip("255.255.255.0")),
            (ip("100.64.0.3"), ip("255.255.255.255")),
        ];
        let to: Vec<String> = targets(8099, &ifs).iter().map(ToString::to_string).collect();
        assert_eq!(to, ["255.255.255.255:8099", "192.168.1.255:8099", "10.8.255.255:8099"]);
        let (a, b, c): (IpAddr, IpAddr, IpAddr) =
            ("192.168.1.7".parse().unwrap(), "10.8.0.2".parse().unwrap(), "192.168.1.8".parse().unwrap());
        assert!(one_machine((a, &[a, b]), (b, &[a, b])), "one machine on two subnets");
        assert!(!one_machine((a, &[a]), (c, &[c])), "two machines never merge");
        assert!(!one_machine((a, &[]), (b, &[a, b])), "one that sends no list is listed per address");
    }

    #[test]
    fn a_probe_nobody_took_says_so_and_names_macos_permission() {
        let to = [SocketAddr::from((Ipv4Addr::BROADCAST, 8099))];
        let e = std::io::Error::new(std::io::ErrorKind::HostUnreachable, "No route to host");
        let mac = unsent(&to, &e, "macos");
        assert!(mac.contains("255.255.255.255:8099 (No route to host): nobody was asked"), "{mac}");
        assert!(mac.contains("Local Network permission"));
        assert!(!unsent(&to, &e, "linux").contains("Local Network"));
    }

    #[tokio::test]
    async fn a_probe_no_target_took_is_an_error_not_no_apps() {
        let nowhere = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0));
        let e = probe(&[nowhere], Duration::from_millis(50)).await.unwrap_err();
        assert!(e.to_string().contains("nobody was asked"), "{e}");
    }

    #[test]
    fn only_the_lan_may_ask() {
        assert!(may_ask("192.168.1.4".parse().unwrap()));
        assert!(may_ask("127.0.0.1".parse().unwrap()));
        assert!(!may_ask("203.0.113.8".parse().unwrap()));
    }

    #[tokio::test]
    async fn a_probe_finds_the_apps_that_answer() {
        let port = UdpSocket::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap().port();
        let app =
            |name: &str| Found { name: name.into(), port: 9000, title: None, needs_auth: true, tls: false, host: None };
        tokio::spawn(respond(port, vec![app("todo"), app("notes")]));
        tokio::time::sleep(Duration::from_millis(50)).await;
        // A second app on this machine can bind the same port.
        assert!(shared_socket(port).is_ok());
        let to = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let mut names: Vec<_> =
            probe(&[to], Duration::from_millis(300)).await.unwrap().into_iter().map(|f| f.name).collect();
        names.sort();
        assert_eq!(names, ["notes", "todo"]);
    }
}

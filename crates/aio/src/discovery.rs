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

fn reply(app: &Found, nonce: Option<&str>) -> Vec<u8> {
    let mut v = json!({ "name": app.name, "port": app.port, "needsAuth": app.needs_auth, "tls": app.tls });
    if let Some(t) = &app.title {
        v["title"] = json!(t);
    }
    if let Some(n) = nonce {
        v["nonce"] = json!(n);
    }
    format!("AIO1 {v}").into_bytes()
}

fn parse_reply(msg: &[u8], host: IpAddr) -> Option<(Found, Option<String>)> {
    let v: Value = serde_json::from_slice(msg.strip_prefix(b"AIO1 ")?).ok()?;
    let found = Found {
        name: v["name"].as_str()?.to_string(),
        port: u16::try_from(v["port"].as_u64()?).ok().filter(|p| *p > 0)?,
        title: v["title"].as_str().map(str::to_string),
        needs_auth: v["needsAuth"].as_bool().unwrap_or(true),
        tls: v["tls"].as_bool().unwrap_or(false),
        host: Some(host),
    };
    Some((found, v["nonce"].as_str().map(str::to_string)))
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
            _ = sock.send_to(&reply(app, nonce), from).await;
        }
    }
}

/// Ask `targets` and collect answers for `wait`; each app once (`host:port:name`).
pub async fn probe(targets: &[SocketAddr], wait: Duration) -> std::io::Result<Vec<Found>> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    sock.set_broadcast(true)?;
    let nonce = crate::uuid_v4().simple().to_string();
    let msg = format!("AIO_DISCOVER? v1 {nonce}");
    for t in targets {
        _ = sock.send_to(msg.as_bytes(), t).await;
    }
    let mut found: Vec<Found> = Vec::new();
    let mut buf = [0u8; 2048];
    let end = tokio::time::Instant::now() + wait;
    while let Ok(Ok((n, from))) = tokio::time::timeout_at(end, sock.recv_from(&mut buf)).await {
        let Some((f, echo)) = parse_reply(&buf[..n], from.ip()) else {
            continue;
        };
        if echo.as_deref() == Some(&nonce)
            && !found.iter().any(|g| (g.host, g.port, &g.name) == (f.host, f.port, &f.name))
        {
            found.push(f);
        }
    }
    Ok(found)
}

/// Ask the whole LAN (`255.255.255.255`) on `port`.
pub async fn probe_lan(port: u16) -> std::io::Result<Vec<Found>> {
    probe(&[SocketAddr::from((Ipv4Addr::BROADCAST, port))], WAIT).await
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
        let (back, nonce) = parse_reply(&reply(&app, Some("n1")), host).unwrap();
        assert_eq!(back, Found { host: Some(host), ..app });
        assert_eq!(nonce.as_deref(), Some("n1"));
        assert!(parse_reply(br#"AIO1 {"name":"x","port":0}"#, host).is_none());
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

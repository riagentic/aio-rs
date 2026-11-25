//! `[capabilities] network` (aio: the least-privilege `--allow-net` set): the
//! hosts aio's outbound requests may reach (the sync link, pairing, sign-in,
//! OIDC, peers, updates, feedback). This machine is always reachable; any
//! other host the list doesn't name is refused before a byte is sent, saying
//! how to allow it. An entry is a host (`api.example.org`, any port), a host
//! and port (`10.0.0.5:8443`), `*.example.org` (its subdomains), `lan` (this
//! network: private and link-local addresses, `.local` names) or `*` (any).
//! A client's server the person picked ([`grant`]: the connect page, a
//! pairing, `--server-url`, `[client] server`) is reached without one: the
//! pick is the consent, for that host and port alone (as aio's thin client).
//!
//! Enforced from [`set`] on (`aio::boot` does, from `aio.toml`); a process
//! that never sets it (aio's own tools) is not an app and is not held.
//! aio's runtime download (the pinned Electron, checked by SHA-256) is aio
//! installing itself, not the app reaching out, and is not held either.
use std::sync::RwLock;

static NETWORK: RwLock<Option<Vec<String>>> = RwLock::new(None);
/// The servers the person picked, as (host, port).
static GRANTED: RwLock<Vec<(String, Option<u16>)>> = RwLock::new(Vec::new());

/// Hold this process's outbound requests to `network` from now on.
pub fn set(network: &[String]) {
    *NETWORK.write().unwrap_or_else(|e| e.into_inner()) = Some(network.to_vec());
}

/// This process's list ([`set`]); `None`: not held (not an app).
pub fn network() -> Option<Vec<String>> {
    NETWORK.read().unwrap_or_else(|e| e.into_inner()).clone()
}

static USB: RwLock<Option<Vec<String>>> = RwLock::new(None);

/// Hold this process's USB devices (`aio::hw`) to `usb` (`[capabilities]
/// usb`) from now on: any other is invisible to it.
pub fn set_usb(usb: &[String]) {
    *USB.write().unwrap_or_else(|e| e.into_inner()) = Some(usb.to_vec());
}

/// May this process see USB device `vendor:product` ([`set_usb`]; not
/// held: any)?
#[cfg(feature = "usb")]
pub(crate) fn usb_allows(vendor: u16, product: u16) -> bool {
    USB.read().unwrap_or_else(|e| e.into_inner()).as_deref().is_none_or(|u| aio_config::capabilities::allows_usb(u, vendor, product))
}

/// A UI test's hold (`TestUi`): this machine and `hosts`, besides the hosts
/// the test binary's other mounts (or an app it booted) allowed.
#[cfg(all(feature = "kit", feature = "testing"))]
pub(crate) fn hold_for_tests(hosts: &[String]) {
    NETWORK.write().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().extend(hosts.iter().cloned());
}

pub use aio_config::capabilities::{allows, bad_entry, host_port};
/// What a refused request says: what it was, and the line that allows it.
pub fn refused(what: &str, host: &str) -> String {
    format!(
        "{what}: {host} is not in this app's [capabilities] network, so aio does not reach it. If it should, \
         add \"{host}\" to network in aio.toml"
    )
}

/// `Err` (`PermissionDenied`) when `network` (`None`: not held) doesn't let
/// aio reach `target` (a URL or `host:port`), for `what` (`"sync"`). Pure.
pub fn held(network: Option<&[String]>, what: &str, target: &str) -> std::io::Result<()> {
    let (host, port) = host_port(target);
    match network {
        Some(n) if !allows(n, &host, port) => {
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, refused(what, &host)))
        }
        _ => Ok(()),
    }
}

/// [`held`] by this process's list ([`set`]), a [`grant`]ed server let through.
pub fn check(what: &str, target: &str) -> std::io::Result<()> {
    if GRANTED.read().unwrap_or_else(|e| e.into_inner()).contains(&host_port(target)) {
        return Ok(());
    }
    held(NETWORK.read().unwrap_or_else(|e| e.into_inner()).as_deref(), what, target)
}

/// Let this process reach `target` (`host:port`, a URL), the server a person
/// picked for this client: that host and port alone, `network` aside.
pub fn grant(target: &str) {
    let mut g = GRANTED.write().unwrap_or_else(|e| e.into_inner());
    let at = host_port(target);
    if !g.contains(&at) {
        g.push(at);
    }
}

/// Take back a [`grant`]: a pick that came to nothing.
#[cfg(feature = "tls")]
pub(crate) fn revoke(target: &str) {
    let at = host_port(target);
    GRANTED.write().unwrap_or_else(|e| e.into_inner()).retain(|g| *g != at);
}

/// A `ureq` resolver that [`check`]s every host first, redirects too.
#[cfg(feature = "outbound")]
#[derive(Debug)]
struct Held(&'static str);

#[cfg(feature = "outbound")]
impl ureq::unversioned::resolver::Resolver for Held {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let host = uri.authority().map_or("", |a| a.as_str());
        check(self.0, host).map_err(ureq::Error::Io)?;
        ureq::unversioned::resolver::DefaultResolver::default().resolve(uri, config, timeout)
    }
}

/// An agent with `config` whose every request is [`check`]ed, for `what`.
#[cfg(feature = "outbound")]
pub fn agent(what: &'static str, config: ureq::config::Config) -> ureq::Agent {
    ureq::Agent::with_parts(config, ureq::unversioned::transport::DefaultConnector::default(), Held(what))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_target_is_reduced_to_its_host_and_port() {
        assert_eq!(host_port("https://Api.Example.org/v1?q=1"), ("api.example.org".into(), None));
        assert_eq!(host_port("wss://u:p@h.example:8443/ws"), ("h.example".into(), Some(8443)));
        assert_eq!(host_port("10.0.0.5:8443"), ("10.0.0.5".into(), Some(8443)));
        assert_eq!(host_port("[::1]:80"), ("::1".into(), Some(80)));
        assert_eq!(host_port("fe80::1"), ("fe80::1".into(), None));
        assert_eq!(host_port("example.org."), ("example.org".into(), None));
    }

    #[test]
    fn a_usb_entry_is_a_vendor_a_product_or_any_and_its_udev_rule_says_so() {
        use aio_config::capabilities::{allows_usb, udev_rule, udev_rules, usb_entry};
        assert_eq!(usb_entry("2c97"), Ok((Some(0x2c97), None)));
        assert_eq!(usb_entry("1209:5342"), Ok((Some(0x1209), Some(0x5342))));
        assert_eq!(usb_entry("ABCD:00ff"), Ok((Some(0xabcd), Some(0xff))), "hex either case");
        assert_eq!(usb_entry("*"), Ok((None, None)));
        for bad in ["", "2c9", "2c977", "0x2c97", "2c97:", ":0001", "2c97:*", "zzzz", "2c97:0001:1", "+c97"] {
            assert!(usb_entry(bad).unwrap_err().contains("is not a device"), "{bad:?}");
        }
        let usb = n(&["2c97", "1209:5342"]);
        assert!(allows_usb(&usb, 0x2c97, 7) && allows_usb(&usb, 0x1209, 0x5342));
        assert!(!allows_usb(&usb, 0x1209, 0x5343) && !allows_usb(&n(&[]), 0x2c97, 1), "nothing else, none by default");
        assert!(allows_usb(&n(&["*"]), 0x0001, 0x0001) && !allows_usb(&n(&["junk"]), 1, 1));
        assert_eq!(udev_rule(0x2c97, None), r#"SUBSYSTEM=="usb", ATTRS{idVendor}=="2c97", TAG+="uaccess""#);
        assert_eq!(
            udev_rule(0x1209, Some(0x5342)),
            r#"SUBSYSTEM=="usb", ATTRS{idVendor}=="1209", ATTRS{idProduct}=="5342", TAG+="uaccess""#
        );
        let file = udev_rules("wallet", &n(&["*", "2c97", "1209:5342"])).unwrap();
        assert!(file.starts_with("# wallet: ") && file.ends_with("TAG+=\"uaccess\"\n"), "{file}");
        assert_eq!(file.lines().count(), 3, "a comment, then one rule per vendor or product: {file}");
        assert_eq!(udev_rules("wallet", &n(&["*"])), None, "any device: no rule (too wide)");
    }

    #[test]
    fn only_listed_hosts_are_reached_and_this_machine_always() {
        let none = n(&[]);
        for h in ["127.0.0.1", "::1", "localhost", "app.localhost"] {
            assert!(allows(&none, h, Some(80)), "{h} is this machine");
        }
        assert!(!allows(&none, "example.org", None), "nothing else by default");
        let list = n(&["api.example.org", "10.0.0.5:8443", "*.cdn.example"]);
        assert!(allows(&list, "api.example.org", Some(443)));
        assert!(!allows(&list, "evil-api.example.org", Some(443)));
        assert!(allows(&list, "10.0.0.5", Some(8443)) && !allows(&list, "10.0.0.5", Some(22)), "the port it names");
        assert!(allows(&list, "a.b.cdn.example", None) && !allows(&list, "cdn.example", None));
        assert!(!allows(&list, "xcdn.example", None), "a subdomain, not a suffix");
        let lan_ = n(&["lan"]);
        for h in ["192.168.1.9", "10.1.2.3", "172.16.0.1", "169.254.1.1", "fd00::1", "fe80::1", "nas.local"] {
            assert!(allows(&lan_, h, None), "{h} is this network");
        }
        assert!(!allows(&lan_, "203.0.113.8", None) && !allows(&lan_, "example.org", None));
        assert!(allows(&n(&["*"]), "anything.example", None));
        assert!(allows(&n(&["API.Example.org"]), "api.example.org", None), "names ignore case");
    }

    #[test]
    fn entries_are_hosts_and_a_refusal_says_how_to_allow_it() {
        for ok in ["api.example.org", "10.0.0.5:8443", "*.example.org", "lan", "*", "[::1]:80"] {
            assert_eq!(bad_entry(ok), None, "{ok}");
        }
        for bad in ["https://api.example.org", "api.example.org/v1", "", "a*.example.org", "*.", "u@h"] {
            assert!(bad_entry(bad).is_some_and(|e| e.contains("is not a host")), "{bad}");
        }
        assert!(held(None, "sync", "example.org:1").is_ok(), "not held: aio's own tools");
        let e = held(Some(&[]), "sync", "example.org:1").unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(held(Some(&n(&["example.org"])), "sync", "wss://example.org:1/ws").is_ok());
        let r = refused("updates", "dl.example.org");
        assert!(r.starts_with("updates: dl.example.org is not in") && r.contains("add \"dl.example.org\""), "{r}");
    }
}

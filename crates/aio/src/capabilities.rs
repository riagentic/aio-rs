//! `[capabilities] network` (aio: the least-privilege `--allow-net` set): the
//! hosts aio's outbound requests may reach (the sync link, pairing, sign-in,
//! OIDC, peers, updates, feedback). This machine is always reachable; any
//! other host the list doesn't name is refused before a byte is sent, saying
//! how to allow it. An entry is a host (`api.example.org`, any port), a host
//! and port (`10.0.0.5:8443`), `*.example.org` (its subdomains), `lan` (this
//! network: private and link-local addresses, `.local` names) or `*` (any).
//!
//! Enforced from [`set`] on (`aio::boot` does, from `aio.toml`); a process
//! that never sets it (aio's own tools) is not an app and is not held.
//! aio's runtime download (the pinned Electron, checked by SHA-256) is aio
//! installing itself, not the app reaching out, and is not held either.
use std::sync::RwLock;

static NETWORK: RwLock<Option<Vec<String>>> = RwLock::new(None);

/// Hold this process's outbound requests to `network` from now on.
pub fn set(network: &[String]) {
    *NETWORK.write().unwrap_or_else(|e| e.into_inner()) = Some(network.to_vec());
}

/// A UI test's hold (`TestUi`): this machine and `hosts`, besides the hosts
/// the test binary's other mounts (or an app it booted) allowed.
#[cfg(feature = "kit")]
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

/// [`held`] by this process's list ([`set`]).
pub fn check(what: &str, target: &str) -> std::io::Result<()> {
    held(NETWORK.read().unwrap_or_else(|e| e.into_inner()).as_deref(), what, target)
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

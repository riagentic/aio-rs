//! TLS for sync, like aio's auto-TLS (`server/tls.ts`): one root per machine,
//! ECDSA P-256, 10 years, name-constrained (critical) to `localhost`,
//! `.local`, loopback, the private ranges and link-local, so even a stolen
//! root key can't vouch for a public site. Each app gets a fresh leaf at
//! start naming this machine's addresses (that the root may name). Clients
//! pin the root (pairing hands it out), never the OS trust store.
use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DnType, ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer,
    KeyPair, KeyUsagePurpose, NameConstraints, SanType,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::io;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const ROOT_DAYS: u64 = 3650;
const LEAF_DAYS: u64 = 825;

/// The ranges the root may name: `(network, mask)`.
const PERMITTED_V4: [([u8; 4], [u8; 4]); 5] = [
    ([127, 0, 0, 0], [255, 0, 0, 0]),
    ([10, 0, 0, 0], [255, 0, 0, 0]),
    ([172, 16, 0, 0], [255, 240, 0, 0]),
    ([192, 168, 0, 0], [255, 255, 0, 0]),
    ([169, 254, 0, 0], [255, 255, 0, 0]),
];

/// A server's TLS: the root clients pin and the config that serves the leaf.
#[derive(Clone)]
pub struct Tls {
    pub root_pem: String,
    /// The leaf it serves (pairing binds to its fingerprint).
    pub leaf_der: Vec<u8>,
    pub server: Arc<rustls::ServerConfig>,
    /// The operator's own certificate ([`own`]), not aio's local root.
    pub own: bool,
}

fn err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("tls: {e}"))
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn days(n: u64) -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc() + Duration::from_secs(n * 86_400)
}

/// May the root vouch for this address?
pub fn permitted(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => PERMITTED_V4
            .iter()
            .any(|(net, mask)| v4.octets().iter().zip(mask).map(|(a, m)| a & m).eq(net.iter().copied())),
        IpAddr::V6(v6) => {
            v6.is_loopback() || (v6.segments()[0] & 0xffc0) == 0xfe80 || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}

fn root_params() -> CertificateParams {
    let mut p = CertificateParams::default();
    p.distinguished_name.push(DnType::CommonName, "aio-rt local root");
    // One intermediate may sit below it (an offline root, a per-app
    // issuer), under the same name constraints.
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(1));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let mut permitted = vec![GeneralSubtree::DnsName("localhost".into()), GeneralSubtree::DnsName("local".into())];
    permitted.extend(PERMITTED_V4.iter().map(|(n, m)| GeneralSubtree::IpAddress(CidrSubnet::V4(*n, *m))));
    permitted.push(GeneralSubtree::IpAddress(CidrSubnet::V6(std::net::Ipv6Addr::LOCALHOST.octets(), [0xff; 16])));
    let fc = [0xfc, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let fe80 = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let (m7, m10) =
        ([0xfe, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], [0xff, 0xc0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    permitted.push(GeneralSubtree::IpAddress(CidrSubnet::V6(fc, m7)));
    permitted.push(GeneralSubtree::IpAddress(CidrSubnet::V6(fe80, m10)));
    // A name type with no permitted subtree is unconstrained: email names
    // only under `.invalid` (none), as aio.
    permitted.push(GeneralSubtree::Rfc822Name(".invalid".into()));
    p.name_constraints = Some(NameConstraints { permitted_subtrees: permitted, excluded_subtrees: vec![] });
    p.not_before = days(0) - Duration::from_secs(3600);
    p.not_after = days(ROOT_DAYS);
    p
}

/// This machine's root in `dir` (made on first use; the key 0600).
pub fn root(dir: &Path) -> io::Result<(String, KeyPair)> {
    let (cert, key) = (dir.join("root.pem"), dir.join("root-key.pem"));
    if let (Ok(c), Ok(k)) = (std::fs::read_to_string(&cert), std::fs::read_to_string(&key)) {
        return Ok((c, KeyPair::from_pem(&k).map_err(err)?));
    }
    crate::paths::private_dir(dir)?;
    let k = KeyPair::generate().map_err(err)?;
    let c = root_params().self_signed(&k).map_err(err)?.pem();
    // Key first, then cert: a crash between them makes a fresh pair next time.
    crate::paths::write_atomic(&key, k.serialize_pem().as_bytes(), true)?;
    crate::paths::write_atomic(&cert, c.as_bytes(), false)?;
    Ok((c, k))
}

/// The names a leaf carries: `localhost` and the addresses the root may name.
pub fn sans(addrs: impl IntoIterator<Item = IpAddr>) -> Vec<SanType> {
    let mut ips: Vec<IpAddr> = vec![IpAddr::from([127, 0, 0, 1]), IpAddr::from(std::net::Ipv6Addr::LOCALHOST)];
    ips.extend(addrs.into_iter().filter(|ip| permitted(*ip)));
    ips.sort();
    ips.dedup();
    let mut out = vec![SanType::DnsName("localhost".try_into().expect("a valid name"))];
    out.extend(ips.into_iter().map(SanType::IpAddress));
    out
}

/// A server config for app `id`: a fresh leaf, signed by the root in `dir`.
pub fn server(dir: &Path, id: &str) -> io::Result<Tls> {
    let (root_pem, root_key) = root(dir)?;
    // The root is made from these params alone, so they are its issuer (no
    // certificate parser in the binary).
    let issuer = Issuer::new(root_params(), root_key);
    let mut p = CertificateParams::default();
    p.distinguished_name.push(DnType::CommonName, format!("aio-{id}"));
    p.subject_alt_names = sans(local_addrs());
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    p.use_authority_key_identifier_extension = true;
    p.not_before = days(0) - Duration::from_secs(3600);
    p.not_after = days(LEAF_DAYS);
    let key = KeyPair::generate().map_err(err)?;
    let leaf = p.signed_by(&key, &issuer).map_err(err)?;
    let leaf_der = leaf.der().to_vec();
    let chain = vec![leaf.der().clone(), CertificateDer::from_pem_slice(root_pem.as_bytes()).map_err(err)?];
    let key = PrivateKeyDer::from_pem_slice(key.serialize_pem().as_bytes()).map_err(err)?;
    Ok(Tls { root_pem, leaf_der, server: serving(chain, key)?, own: false })
}

/// The operator's own certificate (aio's `--tls-cert`/`--tls-key`): the chain
/// in `cert` (leaf first), its key in `key`, both PEM. Clients that pair pin
/// the chain's last certificate.
pub fn own(cert: &Path, key: &Path) -> io::Result<Tls> {
    let read = |p: &Path| std::fs::read(p).map_err(|e| io::Error::new(e.kind(), format!("tls: {}: {e}", p.display())));
    let (pem, key_pem) = (read(cert)?, read(key)?);
    let chain = CertificateDer::pem_slice_iter(&pem).collect::<Result<Vec<_>, _>>().map_err(err)?;
    let (Some(leaf), Some(top)) = (chain.first(), chain.last()) else {
        return Err(err(format!("{} holds no certificate", cert.display())));
    };
    let (leaf_der, root_pem) = (leaf.to_vec(), pem_of(top));
    let key = PrivateKeyDer::from_pem_slice(&key_pem).map_err(|e| err(format!("{}: {e}", key.display())))?;
    Ok(Tls { root_pem, leaf_der, server: serving(chain, key)?, own: true })
}

fn pem_of(der: &[u8]) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
    format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", lines.join("\n"))
}

/// A config serving `chain` with `key` (a key that isn't the leaf's is refused).
fn serving(chain: Vec<CertificateDer<'static>>, key: PrivateKeyDer<'static>) -> io::Result<Arc<rustls::ServerConfig>> {
    let cfg = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(err)?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(err)?;
    Ok(Arc::new(cfg))
}

/// A client config that trusts only `root_pem`.
pub fn client(root_pem: &str) -> io::Result<Arc<rustls::ClientConfig>> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from_pem_slice(root_pem.as_bytes()).map_err(err)?).map_err(err)?;
    let cfg = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(err)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

/// This machine's IPv4 addresses (none on systems not read yet).
pub fn local_addrs() -> Vec<IpAddr> {
    #[cfg(unix)]
    {
        let mut out = Vec::new();
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: getifaddrs fills `list`, walked read-only, then freed once.
        unsafe {
            if libc::getifaddrs(&mut list) != 0 {
                return out;
            }
            let mut at = list;
            while !at.is_null() {
                let a = (*at).ifa_addr;
                if !a.is_null() && i32::from((*a).sa_family) == libc::AF_INET {
                    let v4 = &*(a as *const libc::sockaddr_in);
                    out.push(IpAddr::from(u32::from_be(v4.sin_addr.s_addr).to_be_bytes()));
                }
                at = (*at).ifa_next;
            }
            libc::freeifaddrs(list);
        }
        out
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_names_only_the_lan() {
        for ip in ["127.0.0.1", "10.1.2.3", "172.31.0.1", "192.168.1.9", "169.254.0.7", "::1", "fe80::1", "fd00::5"] {
            assert!(permitted(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["203.0.113.8", "172.32.0.1", "100.64.0.1", "2001:db8::1"] {
            assert!(!permitted(ip.parse().unwrap()), "{ip}");
        }
        let s = sans(["192.168.1.9".parse().unwrap(), "203.0.113.8".parse().unwrap()]);
        assert_eq!(s.len(), 4, "{s:?}");
    }

    /// A TLS handshake in memory: does a client pinning `root` accept `leaf` for `name`?
    async fn accepts(root: &str, chain: Vec<CertificateDer<'static>>, key: &KeyPair, name: &str) -> bool {
        let key = PrivateKeyDer::from_pem_slice(key.serialize_pem().as_bytes()).unwrap();
        let cfg = rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap();
        let (c, s) = tokio::io::duplex(1 << 16);
        let server =
            tokio::spawn(async move { tokio_rustls::TlsAcceptor::from(Arc::new(cfg)).accept(s).await.is_ok() });
        let name = rustls::pki_types::ServerName::try_from(name.to_string()).unwrap();
        let ok = tokio_rustls::TlsConnector::from(client(root).unwrap()).connect(name, c).await.is_ok();
        server.abort();
        ok
    }

    #[tokio::test]
    async fn a_stolen_root_key_cant_vouch_for_a_public_site() {
        let d = tempfile::tempdir().unwrap();
        let (root_pem, root_key) = root(d.path()).unwrap();
        let issuer = Issuer::new(root_params(), root_key);
        let root_der = CertificateDer::from_pem_slice(root_pem.as_bytes()).unwrap();
        let leaf = |names: Vec<SanType>| {
            let mut p = CertificateParams::default();
            p.subject_alt_names = names;
            let k = KeyPair::generate().unwrap();
            (vec![p.signed_by(&k, &issuer).unwrap().der().clone(), root_der.clone()], k)
        };
        let (chain, k) = leaf(sans([]));
        assert!(accepts(&root_pem, chain, &k, "localhost").await);
        let (chain, k) = leaf(vec![SanType::DnsName("example.com".try_into().unwrap())]);
        assert!(!accepts(&root_pem, chain, &k, "example.com").await);
        let (chain, k) = leaf(vec![SanType::IpAddress("203.0.113.8".parse().unwrap())]);
        assert!(!accepts(&root_pem, chain, &k, "203.0.113.8").await);
        let mail = vec![SanType::DnsName("localhost".try_into().unwrap()), SanType::Rfc822Name("a@example.com".try_into().unwrap())];
        let (chain, k) = leaf(mail);
        assert!(!accepts(&root_pem, chain, &k, "localhost").await, "no email name either");
    }

    #[test]
    fn the_root_is_made_once_and_kept() {
        let d = tempfile::tempdir().unwrap();
        let (a, _) = root(d.path()).unwrap();
        let (b, _) = root(d.path()).unwrap();
        assert_eq!(a, b);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(d.path().join("root-key.pem")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(server(d.path(), "todo").is_ok());
        assert!(client(&a).is_ok());
    }

    #[tokio::test]
    async fn an_operators_own_certificate_is_served_and_pinned() {
        let d = tempfile::tempdir().unwrap();
        // A CA of the operator's (not name-constrained) and a leaf for a public name.
        let mut ca = CertificateParams::default();
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_key = KeyPair::generate().unwrap();
        let ca_pem = ca.self_signed(&ca_key).unwrap().pem();
        let issuer = Issuer::new(ca, ca_key);
        let mut p = CertificateParams::default();
        p.subject_alt_names = vec![SanType::DnsName("app.example.com".try_into().unwrap())];
        let leaf_key = KeyPair::generate().unwrap();
        let leaf = p.signed_by(&leaf_key, &issuer).unwrap();
        let (cert, key) = (d.path().join("chain.pem"), d.path().join("key.pem"));
        std::fs::write(&cert, format!("{}{ca_pem}", leaf.pem())).unwrap();
        std::fs::write(&key, leaf_key.serialize_pem()).unwrap();
        let t = own(&cert, &key).unwrap();
        assert!(t.own && t.leaf_der == leaf.der().to_vec());
        assert_eq!(
            CertificateDer::from_pem_slice(t.root_pem.as_bytes()).unwrap(),
            CertificateDer::from_pem_slice(ca_pem.as_bytes()).unwrap(),
            "clients pin the chain's top"
        );
        let chain =
            CertificateDer::pem_slice_iter(std::fs::read(&cert).unwrap().as_slice()).map(Result::unwrap).collect();
        assert!(accepts(&t.root_pem, chain, &leaf_key, "app.example.com").await);
        // A key that isn't the leaf's, a missing file, a file with no certificate: said.
        std::fs::write(&key, KeyPair::generate().unwrap().serialize_pem()).unwrap();
        assert!(own(&cert, &key).is_err());
        assert!(own(&d.path().join("none.pem"), &key).err().unwrap().to_string().contains("none.pem"));
        std::fs::write(&cert, "not a pem").unwrap();
        assert!(own(&cert, &key).err().unwrap().to_string().contains("holds no certificate"));
    }
}

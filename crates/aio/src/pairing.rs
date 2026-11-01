//! PIN pairing, aio's plain mode (`server/pairing.ts`): the server shows a
//! 6-digit PIN; a new device sends it to `POST /__aio/pair` and gets the
//! app's key. A PIN lives 3 minutes and works once; 8 wrong guesses from one
//! address (one /64 for IPv6) lock that address out, and 20 wrong in all
//! burn the PIN.
//!
//! PAKE mode (`new_pake_pin`, 8 digits, as aio's `pairing: { pake: true }`):
//! the PIN never crosses the network. The two sides run CPace (`aio::cpace`)
//! bound to the TLS leaf the client sees, so a relay with its own certificate
//! learns nothing; a wrong confirmation costs a guess like a wrong PIN.
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const PIN_TTL: Duration = Duration::from_secs(180);
pub const PER_ADDRESS: u32 = 8;
pub const TOTAL: u32 = 20;
pub const OFFER_TTL: Duration = Duration::from_secs(30);
const OFFERS_PER_ADDRESS: usize = 2;
const OFFERS: usize = 64;

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Wrong, expired, used or burnt (one answer for all: no oracle).
    Invalid,
    /// This address guessed wrong too often.
    TooMany,
    /// A PAKE PIN was sent in the clear: refused, and the PIN is burnt.
    PakeOnly,
    /// Too many PAKE exchanges open; try again shortly.
    Busy,
    /// No such PAKE exchange (unknown, expired, answered): nothing charged.
    Unknown,
}

/// The server's answer to CPace message 1.
pub struct Offer {
    pub id: String,
    pub yb: [u8; 32],
    pub adb: Vec<u8>,
}

/// What a confirmed exchange gives: the server's tag and the box key.
pub struct Confirmed {
    pub tb: Vec<u8>,
    pub isk: Vec<u8>,
    pub sid: Vec<u8>,
}

struct Open {
    at: Instant,
    from: IpAddr,
    sid: Vec<u8>,
    keys: crate::cpace::Keys,
}

struct Pin {
    code: String,
    pake: bool,
    made: Instant,
    wrong: u32,
    per: HashMap<IpAddr, u32>,
    offers: HashMap<String, Open>,
}

impl Pin {
    fn locked(&self, b: IpAddr) -> bool {
        self.per.get(&b).copied().unwrap_or(0) >= PER_ADDRESS
    }

    /// One wrong guess from `b`; `true` when that burns the PIN.
    fn wrong_guess(&mut self, b: IpAddr) -> bool {
        self.wrong += 1;
        *self.per.entry(b).or_default() += 1;
        self.wrong >= TOTAL
    }
}

#[derive(Default)]
pub struct Pairing {
    pin: Mutex<Option<Pin>>,
}

/// This process's pairing: the app shows `pairing().new_pin()`.
pub fn pairing() -> Arc<Pairing> {
    static P: OnceLock<Arc<Pairing>> = OnceLock::new();
    P.get_or_init(Arc::default).clone()
}

/// An address as aio counts it: IPv6 by its /64.
fn bucket(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        v4 => v4,
    }
}

impl Pairing {
    /// A fresh 6-digit PIN for plain pairing (any older one stops working).
    pub fn new_pin(&self) -> String {
        self.mint(6, false)
    }

    /// A fresh 8-digit PIN for PAKE pairing: the length tells a client never
    /// to send it.
    pub fn new_pake_pin(&self) -> String {
        self.mint(8, true)
    }

    fn mint(&self, digits: u32, pake: bool) -> String {
        let n = u64::from_le_bytes(crate::cpace::random::<8>());
        let code = format!("{:0w$}", n % 10u64.pow(digits), w = digits as usize);
        *self.slot() = Some(Pin {
            code: code.clone(),
            pake,
            made: Instant::now(),
            wrong: 0,
            per: HashMap::new(),
            offers: HashMap::new(),
        });
        code
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<Pin>> {
        self.pin.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The live PIN at `now` (an expired one is dropped).
    fn live(slot: &mut Option<Pin>, now: Instant) -> Option<&mut Pin> {
        if slot.as_ref().is_some_and(|p| now.duration_since(p.made) > PIN_TTL) {
            *slot = None;
        }
        slot.as_mut()
    }

    /// Check a plain guess from `from` at `now`; a right one uses the PIN up.
    pub fn check(&self, guess: &str, from: IpAddr, now: Instant) -> Result<(), Refused> {
        let mut slot = self.slot();
        let Some(pin) = Self::live(&mut slot, now) else {
            return Err(Refused::Invalid);
        };
        let b = bucket(from);
        if pin.locked(b) {
            return Err(Refused::TooMany);
        }
        let right = crate::sync::same(guess, &pin.code);
        if pin.pake {
            // Sent in the clear: whoever saw it may use it, so it's gone.
            if right || pin.wrong_guess(b) {
                *slot = None;
            }
            return Err(Refused::PakeOnly);
        }
        if right {
            *slot = None;
            return Ok(());
        }
        if pin.wrong_guess(b) {
            *slot = None;
        }
        Err(Refused::Invalid)
    }

    /// CPace message 1 from `from`, on the leaf fingerprint `fp` it was
    /// served with. Costs no guess (Yb says nothing about the PIN).
    pub fn offer(
        &self,
        sid: &[u8; 16],
        ya: &[u8; 32],
        fp: &[u8],
        from: IpAddr,
        now: Instant,
    ) -> Result<Offer, Refused> {
        let mut slot = self.slot();
        let Some(pin) = Self::live(&mut slot, now).filter(|p| p.pake) else {
            return Err(Refused::Invalid);
        };
        let b = bucket(from);
        if pin.locked(b) {
            return Err(Refused::TooMany);
        }
        let r = crate::cpace::respond(&pin.code, fp, sid, ya, crate::cpace::random()).ok_or(Refused::Invalid)?;
        pin.offers.retain(|_, o| now.duration_since(o.at) <= OFFER_TTL);
        let mut mine: Vec<(Instant, String)> =
            pin.offers.iter().filter(|(_, o)| o.from == b).map(|(k, o)| (o.at, k.clone())).collect();
        mine.sort();
        for (_, k) in mine.iter().take((mine.len() + 1).saturating_sub(OFFERS_PER_ADDRESS)) {
            pin.offers.remove(k);
        }
        if pin.offers.len() >= OFFERS {
            return Err(Refused::Busy);
        }
        let id = crate::cpace::b64(&crate::cpace::random::<16>());
        pin.offers.insert(id.clone(), Open { at: now, from: b, sid: sid.to_vec(), keys: r.keys });
        Ok(Offer { id, yb: r.yb, adb: r.adb })
    }

    /// CPace message 2: the client's tag. Right → the PIN is used up; wrong →
    /// one wrong guess. One answer per exchange, right or wrong.
    pub fn confirm(&self, id: &str, ta: &[u8], from: IpAddr, now: Instant) -> Result<Confirmed, Refused> {
        let mut slot = self.slot();
        let Some(pin) = Self::live(&mut slot, now) else {
            return Err(Refused::Unknown);
        };
        let Some(o) = pin.offers.remove(id).filter(|o| now.duration_since(o.at) <= OFFER_TTL) else {
            return Err(Refused::Unknown);
        };
        let b = bucket(from);
        if pin.locked(b) {
            return Err(Refused::TooMany);
        }
        if !crate::sync::same_bytes(ta, &o.keys.ta) {
            if pin.wrong_guess(b) {
                *slot = None;
            }
            return Err(Refused::Invalid);
        }
        *slot = None;
        Ok(Confirmed { tb: o.keys.tb, isk: o.keys.isk, sid: o.sid })
    }
}

// ── The client side ──────────────────────────────────────────────────────

/// What pairing gives a new device (aio's profile).
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct Profile {
    pub name: String,
    /// Where it is reached (an exported profile says; pairing knows already).
    #[serde(default)]
    pub host: Option<String>,
    pub port: u16,
    #[serde(default)]
    pub tls: bool,
    /// The root to pin (PEM), when the server speaks TLS.
    #[serde(default)]
    pub cert: Option<String>,
    /// The app's key.
    pub key: String,
}

/// Accepts any certificate (signatures still checked) and keeps the leaf:
/// pairing is what decides whether it is the server's.
#[derive(Debug)]
struct Seen(Mutex<Vec<u8>>, Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for Seen {
    fn verify_server_cert(
        &self,
        leaf: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = leaf.to_vec();
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &rustls::pki_types::CertificateDer<'_>,
        d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.1.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &rustls::pki_types::CertificateDer<'_>,
        d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.1.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.1.signature_verification_algorithms.supported_schemes()
    }
}

/// A connection to `addr`: TLS if the server speaks it (with the leaf it
/// showed), else plain (CPace then binds to no certificate, on both sides).
async fn connect(addr: &str) -> std::io::Result<(Box<dyn crate::http::Io>, Vec<u8>, String)> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let seen = Arc::new(Seen(Mutex::default(), provider.clone()));
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(std::io::Error::other)?
        .dangerous()
        .with_custom_certificate_verifier(seen.clone())
        .with_no_client_auth();
    let host = addr.rsplit_once(':').map_or(addr, |(h, _)| h).trim_matches(['[', ']']).to_string();
    let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(std::io::Error::other)?;
    crate::capabilities::check("pairing", addr)?;
    let tcp = tokio::net::TcpStream::connect(addr).await?;
    let s: Box<dyn crate::http::Io> = match tokio_rustls::TlsConnector::from(Arc::new(cfg)).connect(name, tcp).await {
        Ok(t) => Box::new(t),
        Err(_) => Box::new(tokio::net::TcpStream::connect(addr).await?),
    };
    let leaf = seen.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Ok((s, leaf, host))
}

/// One `POST /__aio/pair` on a fresh connection; the status, the JSON, the leaf seen.
async fn post(addr: &str, body: &serde_json::Value) -> std::io::Result<(u16, serde_json::Value, Vec<u8>)> {
    let (s, leaf, host) = connect(addr).await?;
    let (status, v) = crate::http::request(s, &host, "/__aio/pair", None, Some(body)).await?;
    Ok((status, v, leaf))
}

fn refused(status: u16, v: &serde_json::Value) -> std::io::Error {
    let kind = if status == 429 { std::io::ErrorKind::QuotaExceeded } else { std::io::ErrorKind::PermissionDenied };
    std::io::Error::new(kind, format!("pairing refused ({status}): {}", v["error"].as_str().unwrap_or("?")))
}

/// Pair with the app at `addr` using the PIN it shows. 8 digits: PAKE (the
/// PIN never leaves this device); 6: plain, as aio.
pub async fn pair(addr: &str, pin: &str) -> std::io::Result<Profile> {
    use crate::cpace;
    use serde_json::json;
    let parse = |v: serde_json::Value| {
        serde_json::from_value::<Profile>(v).map_err(|e| std::io::Error::other(format!("pairing: bad profile: {e}")))
    };
    if !pin.bytes().all(|b| b.is_ascii_digit()) || !(pin.len() == 6 || pin.len() == 8) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "a pairing code has 6 or 8 digits"));
    }
    if pin.len() == 6 {
        let (status, v, _) = post(addr, &json!({ "pin": pin })).await?;
        return if status == 200 { parse(v) } else { Err(refused(status, &v)) };
    }
    // The certificate seen goes into the generator: learn it first, then
    // check every connection shows the same one.
    let (_, leaf, _) = connect(addr).await?;
    let fp = cpace::fingerprint(&leaf);
    let sid: [u8; 16] = cpace::random();
    let st = cpace::start(pin, &fp, cpace::random(), sid);
    let (status, v, seen) =
        post(addr, &json!({ "cpace": cpace::SUITE, "sid": cpace::b64(&sid), "Ya": cpace::b64(&st.ya) })).await?;
    if status != 200 {
        return Err(refused(status, &v));
    }
    let bad = || std::io::Error::other("pairing: the answer doesn't verify (a wrong code, or something in between)");
    let (Some(yb), Some(id), Some(adb)) = (cpace::unb64::<32>(&v["Yb"]), v["id"].as_str(), cpace::unb64_vec(&v["ADb"]))
    else {
        return Err(bad());
    };
    if seen != leaf || adb != fp {
        return Err(bad());
    }
    let keys = cpace::finish(&st, &yb, &fp).ok_or_else(bad)?;
    let (status, v, seen) = post(addr, &json!({ "cpace": cpace::SUITE, "id": id, "Ta": cpace::b64(&keys.ta) })).await?;
    if status != 200 {
        return Err(refused(status, &v));
    }
    let tb = cpace::unb64::<64>(&v["Tb"]).ok_or_else(bad)?;
    if seen != leaf || !crate::sync::same_bytes(&tb, &keys.tb) {
        return Err(bad());
    }
    let text = cpace::unb64_vec(&v["box"]).and_then(|b| cpace::open(&keys.isk, &sid, &b)).ok_or_else(bad)?;
    parse(serde_json::from_str(&text).map_err(|_| bad())?)
}

pub use crate::sync::Serving;

fn serving(data: &std::path::Path) -> std::io::Result<Serving> {
    std::fs::read_to_string(data.join("serving.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or_else(
        || {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "the app isn't serving (no serving.json): set [server] listen and start it",
            )
        },
    )
}

/// The address others reach a server listening on `host` at: this
/// machine's first LAN address for a wildcard, else `host` (pure).
pub fn reached_at(host: Option<&str>, lan: &[std::net::IpAddr]) -> String {
    let host = host.unwrap_or("127.0.0.1");
    let wildcard = host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_unspecified());
    match lan.iter().find(|ip| !ip.is_loopback()) {
        Some(ip) if wildcard => ip.to_string(),
        _ if wildcard => "127.0.0.1".into(),
        _ => host.into(),
    }
}

/// The running app `name`'s profile (aio's `.aioapp`, its JSON as is): host,
/// port, the root to pin, the key. Whoever holds it can connect forever:
/// it is a credential. `true` with it: only this machine can use it.
pub fn local_profile(data: &std::path::Path, name: &str) -> std::io::Result<(serde_json::Value, bool)> {
    let s = serving(data)?;
    let cert = match s.tls {
        true => Some(std::fs::read_to_string(crate::paths::tool_home().join("tls/root.pem"))?),
        false => None,
    };
    let host = reached_at(s.host.as_deref(), &crate::tls::local_addrs());
    let local = host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()) || host == "localhost";
    let p = serde_json::json!({
        "aio": 1, "name": name, "title": name, "host": host, "port": s.port, "tls": s.tls, "cert": cert,
        "key": crate::config::token(data)?,
    });
    Ok((p, local))
}

/// Import a profile (`.aioapp` text) for client mode, as pairing does;
/// where it links to.
pub fn import(data: &std::path::Path, app: &str, text: &str) -> std::io::Result<String> {
    let p: Profile = serde_json::from_str(text)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("not an aio profile: {e}")))?;
    if p.name != app {
        return Err(std::io::Error::other(format!("the profile is of {}, not {app}", p.name)));
    }
    let addr = format!("{}:{}", p.host.as_deref().unwrap_or("127.0.0.1"), p.port);
    crate::paths::private_dir(data)?;
    save(data, &addr, &p)?;
    Ok(addr)
}

/// Ask the app serving from `data` on this machine for a new PIN (8 digits,
/// or 6 when `plain`). It needs the app's key, which only its user can read.
pub async fn mint(data: &std::path::Path, plain: bool) -> std::io::Result<String> {
    let serving = serving(data)?;
    let key = crate::config::token(data)?;
    let addr = format!("127.0.0.1:{}", serving.port);
    let tcp = tokio::net::TcpStream::connect(&addr)
        .await
        .map_err(|e| std::io::Error::new(e.kind(), format!("the app isn't running on {addr}: {e}")))?;
    let s: Box<dyn crate::http::Io> = if serving.tls {
        let root = std::fs::read_to_string(crate::paths::tool_home().join("tls/root.pem"))?;
        let name = rustls::pki_types::ServerName::try_from("127.0.0.1").map_err(std::io::Error::other)?;
        Box::new(tokio_rustls::TlsConnector::from(crate::tls::client(&root)?).connect(name, tcp).await?)
    } else {
        Box::new(tcp)
    };
    let (status, v) =
        crate::http::request(s, "127.0.0.1", "/__aio/pin", Some(&key), Some(&serde_json::json!({ "plain": plain }))).await?;
    match v["pin"].as_str() {
        Some(pin) if status == 200 => Ok(pin.to_string()),
        _ => Err(refused(status, &v)),
    }
}

// ── Pin proof (aio's `/__aio/pin-proof`) ─────────────────────────────────────
//
// An aio client whose pin no longer covers the certificate it sees asks the
// server to prove it holds the key, on that certificate, before sending it:
// client → {nonce, tag = HMAC(key, "…:c:" nonce)}, server → {proof =
// HMAC(key, "…:s:" nonce ":" cert), cert}. Only a caller that shows the key
// is answered, so nobody gets material to guess a weak key against. (aio-rs's
// own clients pin the root, so a new leaf needs no proof.)

const PROOF: &str = "aio-pin-proof-v1";

fn hmac_hex(key: &str, msg: &str) -> String {
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_bytes());
    ring::hmac::sign(&k, msg.as_bytes()).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// What a client sends beside its nonce: it holds the key.
pub fn pin_proof_tag(key: &str, nonce: &str) -> String {
    hmac_hex(key, &format!("{PROOF}:c:{nonce}"))
}

/// What the server answers: it holds the key, and serves `cert` (compared
/// without whitespace: PEM line breaks carry nothing).
pub fn pin_proof_for(key: &str, nonce: &str, cert: &str) -> String {
    let cert: String = cert.split_whitespace().collect();
    hmac_hex(key, &format!("{PROOF}:s:{nonce}:{cert}"))
}

/// The answer to one `POST /__aio/pin-proof` body: `{proof, cert}`, or the
/// status and why (401: a tag that isn't this key's). Pure.
pub fn answer_pin_proof(body: &[u8], key: &str, cert: &str) -> Result<serde_json::Value, (u16, String)> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| (400, "invalid request".to_string()))?;
    let nonce = v["nonce"].as_str().filter(|n| {
        (32..=128).contains(&n.len()) && n.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    });
    let Some(nonce) = nonce else { return Err((400, "nonce must be 32–128 lowercase hex characters".into())) };
    let Some(tag) = v["tag"].as_str() else { return Err((400, "tag must be a string".into())) };
    if !crate::sync::same(tag, &pin_proof_tag(key, nonce)) {
        return Err((401, "the tag is not this app's key's".into()));
    }
    Ok(serde_json::json!({ "proof": pin_proof_for(key, nonce, cert), "cert": cert }))
}

/// The certificate file a server serves, as aio does: the leaf, then its root.
pub fn served_cert(leaf_der: &[u8], root_pem: &str) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(leaf_der);
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
    format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n{root_pem}", lines.join("\n"))
}

/// Keep what pairing gave, for client mode: `data/paired.json` (0600).
pub fn save(data: &std::path::Path, addr: &str, p: &Profile) -> std::io::Result<()> {
    let host = addr.rsplit_once(':').map_or(addr, |(h, _)| h);
    let link = serde_json::json!({ "addr": format!("{host}:{}", p.port), "token": p.key, "root": p.cert });
    // A new pairing replaces the old one.
    crate::paths::write_atomic(&data.join("paired.json"), link.to_string().as_bytes(), true)
}

/// The link pairing saved in `data`, if any.
pub fn saved(data: &std::path::Path) -> Option<crate::sync::Link> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(data.join("paired.json")).ok()?).ok()?;
    Some(crate::sync::Link {
        addr: v["addr"].as_str()?.into(),
        token: v["token"].as_str()?.into(),
        root: v["root"].as_str().map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pin_proof_answers_only_a_key_holder_on_the_served_cert() {
        let (key, nonce, cert) =
            ("k".repeat(32), "ab".repeat(16), "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n");
        let body = |tag: &str| serde_json::json!({ "nonce": nonce, "tag": tag }).to_string().into_bytes();
        let ok = answer_pin_proof(&body(&pin_proof_tag(&key, &nonce)), &key, cert).unwrap();
        assert_eq!(ok["cert"], cert);
        assert_eq!(ok["proof"], pin_proof_for(&key, &nonce, cert));
        // Line breaks carry nothing; another cert or key is another proof.
        assert_eq!(pin_proof_for(&key, &nonce, cert), pin_proof_for(&key, &nonce, &cert.replace('\n', "\r\n")));
        assert_ne!(pin_proof_for(&key, &nonce, cert), pin_proof_for(&key, &nonce, "other"));
        assert_ne!(pin_proof_tag(&key, &nonce), pin_proof_tag("other", &nonce));
        assert_eq!(answer_pin_proof(&body(&pin_proof_tag("wrong", &nonce)), &key, cert).unwrap_err().0, 401);
        for bad in [
            serde_json::json!({ "nonce": "AB".repeat(16), "tag": "x" }),
            serde_json::json!({ "nonce": "ab", "tag": "x" }),
            serde_json::json!({ "nonce": "a:".repeat(16), "tag": "x" }),
            serde_json::json!({ "nonce": nonce }),
        ] {
            assert_eq!(answer_pin_proof(bad.to_string().as_bytes(), &key, cert).unwrap_err().0, 400, "{bad}");
        }
        assert_eq!(answer_pin_proof(b"not json", &key, cert).unwrap_err().0, 400);
        let pem = served_cert(&[1, 2, 3], "ROOT\n");
        assert_eq!(pem, "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\nROOT\n");
    }

    #[test]
    fn a_profile_names_where_others_reach_the_app() {
        let lan: Vec<std::net::IpAddr> = vec![[127, 0, 0, 1].into(), [192, 168, 1, 20].into()];
        assert_eq!(reached_at(Some("0.0.0.0"), &lan), "192.168.1.20");
        assert_eq!(reached_at(Some("::"), &lan), "192.168.1.20");
        assert_eq!(reached_at(Some("0.0.0.0"), &lan[..1]), "127.0.0.1");
        assert_eq!(reached_at(Some("10.0.0.5"), &lan), "10.0.0.5");
        assert_eq!(reached_at(None, &lan), "127.0.0.1");
    }

    #[test]
    fn an_exported_profile_imports_as_a_pairing() {
        let (server, client) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let serving = Serving { port: 8443, tls: false, host: Some("10.0.0.5".into()) };
        std::fs::write(server.path().join("serving.json"), serde_json::to_string(&serving).unwrap()).unwrap();
        std::fs::write(server.path().join("sync.token"), "k".repeat(40)).unwrap();
        let (p, local) = local_profile(server.path(), "todo").unwrap();
        assert!(!local);
        assert_eq!(
            (p["aio"].as_u64(), p["host"].as_str(), p["port"].as_u64()),
            (Some(1), Some("10.0.0.5"), Some(8443))
        );
        assert_eq!(
            (p["tls"].as_bool(), p["cert"].is_null(), p["key"].as_str()),
            (Some(false), true, Some(&*"k".repeat(40)))
        );
        let e = import(client.path(), "notes", &p.to_string()).unwrap_err();
        assert!(e.to_string().contains("of todo, not notes"), "{e}");
        assert!(import(client.path(), "todo", "{}").is_err());
        assert_eq!(import(client.path(), "todo", &p.to_string()).unwrap(), "10.0.0.5:8443");
        let link = saved(client.path()).unwrap();
        assert_eq!((link.addr.as_str(), link.token.len(), link.root), ("10.0.0.5:8443", 40, None));
        let (_, local) = {
            let s = Serving { host: Some("127.0.0.1".into()), ..serving };
            std::fs::write(server.path().join("serving.json"), serde_json::to_string(&s).unwrap()).unwrap();
            local_profile(server.path(), "todo").unwrap()
        };
        assert!(local, "a loopback app's profile works here only");
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn a_pin_works_once() {
        let p = Pairing::default();
        let pin = p.new_pin();
        assert_eq!(pin.len(), 6);
        assert_eq!(p.check(&pin, ip("10.0.0.2"), Instant::now()), Ok(()));
        assert_eq!(p.check(&pin, ip("10.0.0.2"), Instant::now()), Err(Refused::Invalid));
    }

    #[test]
    fn pake_pairs_without_sending_the_pin() {
        use crate::cpace;
        let p = Pairing::default();
        let pin = p.new_pake_pin();
        assert_eq!(pin.len(), 8);
        let (fp, from, now) = (cpace::fingerprint(b"leaf"), ip("10.0.0.2"), Instant::now());
        // Right PIN, same certificate view: both sides end with the same keys.
        let st = cpace::start(&pin, &fp, cpace::random(), cpace::random());
        let offer = p.offer(&st.sid, &st.ya, &fp, from, now).unwrap();
        let mine = cpace::finish(&st, &offer.yb, &offer.adb).unwrap();
        let ok = p.confirm(&offer.id, &mine.ta, from, now).unwrap();
        assert_eq!((ok.tb, ok.isk), (mine.tb, mine.isk));
        // Used up.
        assert!(p.offer(&st.sid, &st.ya, &fp, from, now).is_err());
    }

    #[test]
    fn pake_refuses_wrong_pins_relays_and_the_pin_in_the_clear() {
        use crate::cpace;
        let p = Pairing::default();
        let pin = p.new_pake_pin();
        let (fp, from, now) = (cpace::fingerprint(b"leaf"), ip("10.0.0.2"), Instant::now());
        let st = cpace::start("00000000", &fp, cpace::random(), cpace::random());
        let offer = p.offer(&st.sid, &st.ya, &fp, from, now).unwrap();
        let keys = cpace::finish(&st, &offer.yb, &offer.adb).unwrap();
        assert!(matches!(p.confirm(&offer.id, &keys.ta, from, now), Err(Refused::Invalid)));
        // One answer per exchange.
        assert!(matches!(p.confirm(&offer.id, &keys.ta, from, now), Err(Refused::Unknown)));
        // A relay that shows the client its own certificate.
        let st = cpace::start(&pin, &cpace::fingerprint(b"relay"), cpace::random(), cpace::random());
        let offer = p.offer(&st.sid, &st.ya, &fp, from, now).unwrap();
        let keys = cpace::finish(&st, &offer.yb, &offer.adb).unwrap();
        assert!(matches!(p.confirm(&offer.id, &keys.ta, from, now), Err(Refused::Invalid)));
        // Exchanges expire.
        let st = cpace::start(&pin, &fp, cpace::random(), cpace::random());
        let offer = p.offer(&st.sid, &st.ya, &fp, from, now).unwrap();
        let keys = cpace::finish(&st, &offer.yb, &offer.adb).unwrap();
        assert!(matches!(
            p.confirm(&offer.id, &keys.ta, from, now + OFFER_TTL + Duration::from_secs(1)),
            Err(Refused::Unknown)
        ));
        // Sent in the clear, it's refused and burnt.
        assert_eq!(p.check(&pin, from, now), Err(Refused::PakeOnly));
        assert!(p.offer(&st.sid, &st.ya, &fp, from, now).is_err());
    }

    #[test]
    fn a_pin_expires() {
        let p = Pairing::default();
        let pin = p.new_pin();
        let later = Instant::now() + PIN_TTL + Duration::from_secs(1);
        assert_eq!(p.check(&pin, ip("10.0.0.2"), later), Err(Refused::Invalid));
    }

    #[test]
    fn guessing_is_limited_per_address_and_in_all() {
        let p = Pairing::default();
        let pin = p.new_pin();
        let wrong = if pin == "000000" { "000001" } else { "000000" };
        for _ in 0..PER_ADDRESS {
            assert_eq!(p.check(wrong, ip("10.0.0.2"), Instant::now()), Err(Refused::Invalid));
        }
        // Locked out, even with the right PIN; the same /64 counts as one.
        assert_eq!(p.check(&pin, ip("10.0.0.2"), Instant::now()), Err(Refused::TooMany));
        for i in 0..PER_ADDRESS {
            _ = p.check(wrong, ip(&format!("fd00::{i}")), Instant::now());
        }
        assert_eq!(p.check(&pin, ip("fd00::99"), Instant::now()), Err(Refused::TooMany));
        // 20 wrong in all burn it for everyone.
        for i in 0..4 {
            _ = p.check(wrong, ip(&format!("10.0.1.{i}")), Instant::now());
        }
        assert_eq!(p.check(&pin, ip("10.0.2.1"), Instant::now()), Err(Refused::Invalid));
    }
}

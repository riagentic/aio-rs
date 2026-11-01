//! Password hashes in aio's format, so an `auth.db` moves between aio and aio-rs:
//! `pbkdf2$sha256$<iterations>$<salt hex>$<hash hex>`, PBKDF2-HMAC-SHA-256
//! with a 16-byte random salt and OWASP's iteration count. The count is in
//! the hash, so it can be raised later without breaking old rows.
use std::num::NonZeroU32;

/// OWASP 2023 for PBKDF2-HMAC-SHA-256.
pub const ITERATIONS: u32 = 600_000;

/// A hash no password matches, checked for unknown ids: a login then takes
/// the same time whether the account exists or not.
pub(crate) fn decoy() -> String {
    format!("pbkdf2$sha256${ITERATIONS}${}${}", "00".repeat(16), "00".repeat(32))
}

pub fn hash(password: &str) -> String {
    let salt: [u8; 16] = super::random();
    format!("pbkdf2$sha256${ITERATIONS}${}${}", super::hex(&salt), super::hex(&derive(password, &salt, ITERATIONS)))
}

/// Does `password` match `stored`? In constant time; a malformed hash matches nothing.
pub fn verify(password: &str, stored: &str) -> bool {
    let parts: Vec<&str> = stored.split('$').collect();
    let [scheme, alg, n, salt, want] = parts[..] else { return false };
    let (Some(n), Some(salt), Some(want)) = (n.parse().ok().and_then(NonZeroU32::new), unhex(salt), unhex(want)) else {
        return false;
    };
    if scheme != "pbkdf2" || alg != "sha256" || want.is_empty() {
        return false;
    }
    ring::pbkdf2::verify(ring::pbkdf2::PBKDF2_HMAC_SHA256, n, &salt, password.as_bytes(), &want).is_ok()
}

fn derive(password: &str, salt: &[u8], n: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    let n = NonZeroU32::new(n).expect("a positive count");
    ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA256, n, salt, password.as_bytes(), &mut out);
    out
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len().is_multiple_of(2) && s.is_ascii())
        .then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_verify_and_keep_aios_format() {
        let h = hash("correct horse");
        assert!(h.starts_with("pbkdf2$sha256$600000$"));
        assert!(verify("correct horse", &h));
        assert!(!verify("correct horsf", &h));
        // Salted: the same password hashes differently.
        assert_ne!(h, hash("correct horse"));
        for bad in ["", "pbkdf2$sha256$0$00$00", "pbkdf2$sha1$1$00$00", "pbkdf2$sha256$1$zz$00", &decoy()] {
            assert!(!verify("correct horse", bad), "{bad}");
        }
    }

    #[test]
    fn a_hash_in_aios_format_verifies() {
        // RFC 7914 §11's PBKDF2-HMAC-SHA-256 vector ("passwd", "salt", 1), as aio stores it.
        let aio_hash = "pbkdf2$sha256$1$73616c74$55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc";
        assert!(verify("passwd", aio_hash));
        assert!(!verify("passwe", aio_hash));
    }
}

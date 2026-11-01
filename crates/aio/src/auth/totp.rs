//! TOTP second factors (RFC 6238 over RFC 4226), as every authenticator app
//! speaks them: HMAC-SHA-1, 30-second steps, 6 digits, base32 secrets.
//! Pure: the time is an argument. Which step an account last used is kept
//! with the account (`Auth`), so a code works once, even across a restart.

pub const STEP_MS: i64 = 30_000;
const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 base32, unpadded.
pub fn base32(bytes: &[u8]) -> String {
    let (mut out, mut acc, mut bits) = (String::new(), 0u32, 0);
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            out.push(B32[((acc >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(B32[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// Base32 back to bytes (any case, padding ignored); `None` if not base32.
pub fn unbase32(s: &str) -> Option<Vec<u8>> {
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0);
    for c in s.trim_end_matches('=').bytes() {
        let i = B32.iter().position(|&x| x == c.to_ascii_uppercase())?;
        acc = (acc << 5) | i as u32;
        bits += 5;
        if bits >= 8 {
            out.push((acc >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

/// A fresh 160-bit secret, in base32 (what an enrolment QR code carries).
pub fn secret() -> String {
    base32(&super::random::<20>())
}

/// The `otpauth://` URI to show as a QR code or paste into the app.
pub fn uri(secret: &str, account: &str, issuer: &str) -> String {
    let (i, a) = (enc(issuer), enc(account));
    format!("otpauth://totp/{i}:{a}?secret={secret}&issuer={i}&algorithm=SHA1&digits=6&period=30")
}

/// The code of `step` (ms / `STEP_MS`).
pub fn code(secret: &[u8], step: i64) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let mac = ring::hmac::sign(&key, &step.to_be_bytes());
    let m = mac.as_ref();
    let o = usize::from(m[m.len() - 1] & 0x0f);
    let bin = u32::from_be_bytes([m[o] & 0x7f, m[o + 1], m[o + 2], m[o + 3]]);
    format!("{:06}", bin % 1_000_000)
}

/// The step `submitted` is the code of, at `now` ± one step (clock skew),
/// newer than `used` (the last step accepted): `None` for a wrong or spent code.
pub fn accept(secret: &[u8], submitted: &str, now: i64, used: i64) -> Option<i64> {
    if submitted.len() != 6 || !submitted.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let t = now.div_euclid(STEP_MS);
    [t, t - 1, t + 1]
        .into_iter()
        .find(|&s| same(code(secret, s).as_bytes(), submitted.as_bytes()))
        .filter(|&s| s > used)
}

/// Equal, in time that doesn't depend on where they differ.
pub(crate) fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |d, (x, y)| d | (x ^ y)) == 0
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_6238_vectors() {
        let k = b"12345678901234567890";
        // RFC 6238 appendix B, SHA-1, the last 6 of its 8 digits.
        for (t, want) in
            [(59, "287082"), (1_111_111_109, "081804"), (1_234_567_890, "005924"), (20_000_000_000, "353130")]
        {
            assert_eq!(code(k, t / 30), want, "t={t}");
        }
    }

    #[test]
    fn base32_round_trips_as_rfc_4648() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(unbase32("mzxw6ytboi======").as_deref(), Some(&b"foobar"[..]));
        assert_eq!(unbase32("MZ1"), None);
        let s = secret();
        assert_eq!((s.len(), unbase32(&s).map(|b| b.len())), (32, Some(20)));
    }

    #[test]
    fn a_code_works_once_within_one_step_of_skew() {
        let k = b"12345678901234567890";
        let now = 1_111_111_109_000;
        let t = now / STEP_MS;
        assert_eq!(accept(k, &code(k, t), now, 0), Some(t));
        assert_eq!(accept(k, &code(k, t - 1), now, 0), Some(t - 1));
        assert_eq!(accept(k, &code(k, t + 1), now, 0), Some(t + 1));
        assert_eq!(accept(k, &code(k, t - 2), now, 0), None);
        // Spent: a step at or below the last one accepted is refused.
        assert_eq!(accept(k, &code(k, t), now, t), None);
        assert_eq!(accept(k, &code(k, t - 1), now, t), None);
        for bad in ["", "12345", "1234567", "12a456", " 12345"] {
            assert_eq!(accept(k, bad, now, 0), None);
        }
    }

    #[test]
    fn uri_names_issuer_and_account() {
        assert_eq!(
            uri("ABC", "ann@example.org", "My App"),
            "otpauth://totp/My%20App:ann%40example.org?secret=ABC&issuer=My%20App&algorithm=SHA1&digits=6&period=30"
        );
    }
}

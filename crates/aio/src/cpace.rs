//! CPace (draft-irtf-cfrg-cpace-21, `CPACE-X25519-SHA512`) and aio's pairing
//! exchange on it (`server/cpace.ts`), byte for byte: the same draft vectors
//! and aio's shared golden vector are the tests.
//!
//! Client = initiator A, server = responder B:
//! - `fp` = SHA-256 of the TLS leaf's DER (empty over plain http)
//! - PRS = the PIN, CI = `lv_cat("aio-pair-v1", fp)`, sid = 16 random bytes
//!   from the client, ADa = "", ADb = fp
//! - 1. C → S `{cpace, sid, Ya}`, S → C `{cpace, id, Yb, ADb}`
//! - 2. C → S `{cpace, id, Ta}`, S → C `{Tb, box}`
//! - mac_key = SHA-512("CPaceMac" ‖ sid ‖ ISK); Ta/Tb = HMAC-SHA512 over
//!   `lv_cat(Y, AD)`; box = iv ‖ AES-256-GCM(HKDF-SHA256(ISK, sid,
//!   "aio-pair-v1 profile"), profile JSON)
//!
//! Elligator2 is the draft's Appendix A.5 on fixed 51-bit limbs, in constant
//! time (aio uses big integers; the outputs are the same bits).
use ring::{aead, digest, hkdf, hmac};

pub const SUITE: &str = "CPACE-X25519-SHA512";

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// LEB128 length, then the bytes (draft A.1.1).
pub fn prepend_len(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 2);
    let mut n = data.len();
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        out.push(if n == 0 { b } else { b | 0x80 });
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(data);
    out
}

pub fn lv_cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.iter().flat_map(|p| prepend_len(p)).collect()
}

pub fn generator_string(dsi: &[u8], prs: &[u8], ci: &[u8], sid: &[u8], s_in_bytes: usize) -> Vec<u8> {
    let zpad = s_in_bytes.saturating_sub(1 + prepend_len(prs).len() + prepend_len(dsi).len());
    lv_cat(&[dsi, prs, &vec![0; zpad], ci, sid])
}

/// GF(2^255 − 19) in five 51-bit limbs. No branch or index depends on a
/// value, so the PIN-derived input leaves no trace in the timing.
#[derive(Clone, Copy)]
struct Fe([u64; 5]);

const MASK: u64 = (1 << 51) - 1;
/// 4p by limb: added before a subtraction so no limb goes below zero.
const P4: [u64; 5] = [4 * (MASK - 18), 4 * MASK, 4 * MASK, 4 * MASK, 4 * MASK];
/// p − 2 (an inverse) and (p − 1)/2 (the Legendre symbol), little-endian.
const P_MINUS_2: [u64; 4] = [u64::MAX - 20, u64::MAX, u64::MAX, u64::MAX >> 1];
const HALF_P: [u64; 4] = [u64::MAX - 9, u64::MAX, u64::MAX, u64::MAX >> 2];

impl Fe {
    fn small(n: u64) -> Fe {
        Fe([n, 0, 0, 0, 0])
    }

    /// The low 255 bits.
    fn from_bytes(b: &[u8; 32]) -> Fe {
        let w = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().expect("8 bytes"));
        Fe([w(0) & MASK, (w(6) >> 3) & MASK, (w(12) >> 6) & MASK, (w(19) >> 1) & MASK, (w(24) >> 12) & MASK])
    }

    /// Limbs carried down to 51 bits (52 for the second), 2^255 folded in as 19.
    fn carry(c: [u128; 5]) -> Fe {
        let (mut l, mut k) = ([0u64; 5], 0u128);
        for i in 0..5 {
            let t = c[i] + k;
            l[i] = t as u64 & MASK;
            k = t >> 51;
        }
        let t = u128::from(l[0]) + k * 19;
        l[0] = t as u64 & MASK;
        l[1] += (t >> 51) as u64;
        Fe(l)
    }

    /// self^e for a public exponent.
    fn pow(self, e: &[u64; 4]) -> Fe {
        (0..255).rev().fold(Fe::small(1), |r, i| if e[i / 64] >> (i % 64) & 1 == 1 { r * r * self } else { r * r })
    }

    /// The canonical little-endian bytes (the value mod p).
    fn to_bytes(self) -> [u8; 32] {
        let mut l = Fe::carry(self.0.map(u128::from)).0;
        // q = 1 iff the value ≥ p; adding 19q and dropping bit 255 subtracts qp.
        let q = l[1..].iter().fold((l[0] + 19) >> 51, |q, x| (x + q) >> 51);
        l[0] += 19 * q;
        for i in 0..4 {
            l[i + 1] += l[i] >> 51;
            l[i] &= MASK;
        }
        l[4] &= MASK;
        let (mut out, mut acc, mut bits, mut k) = ([0u8; 32], 0u128, 0, 0);
        for x in l {
            acc |= u128::from(x) << bits;
            bits += 51;
            while bits >= 8 {
                out[k] = acc as u8;
                (acc, bits, k) = (acc >> 8, bits - 8, k + 1);
            }
        }
        out[31] = acc as u8;
        out
    }
}

impl std::ops::Add for Fe {
    type Output = Fe;
    fn add(self, b: Fe) -> Fe {
        Fe::carry(std::array::from_fn(|i| u128::from(self.0[i] + b.0[i])))
    }
}

impl std::ops::Sub for Fe {
    type Output = Fe;
    fn sub(self, b: Fe) -> Fe {
        Fe::carry(std::array::from_fn(|i| u128::from(self.0[i] + P4[i] - b.0[i])))
    }
}

impl std::ops::Mul for Fe {
    type Output = Fe;
    fn mul(self, b: Fe) -> Fe {
        let mut c = [0u128; 5];
        for i in 0..5 {
            for j in 0..5 {
                let bj = if i + j < 5 { b.0[j] } else { b.0[j] * 19 };
                c[(i + j) % 5] += u128::from(self.0[i]) * u128::from(bj);
            }
        }
        Fe::carry(c)
    }
}

/// Draft Appendix A.5 `elligator2(r, q, A, 255)`, with Z = 2, in constant time.
fn elligator2(r: &[u8; 32]) -> [u8; 32] {
    let (r, a, one) = (Fe::from_bytes(r), Fe::small(486_662), Fe::small(1));
    // v = -A / (1 + Z r²); 0 has no inverse and stays 0, as x^(p-2) does
    let v = Fe::small(0) - a * (one + Fe::small(2) * r * r).pow(&P_MINUS_2);
    // eps = (v³ + A v² + v)^((p-1)/2): 1 or p-1 (or 0)
    let eps = (v * v * v + a * v * v + v).pow(&HALF_P);
    // x = eps·v − (1 − eps)·A/2
    (eps * v - (one - eps) * a * Fe::small(2).pow(&P_MINUS_2)).to_bytes()
}

fn sha512(b: &[u8]) -> Vec<u8> {
    digest::digest(&digest::SHA512, b).as_ref().to_vec()
}

pub fn calculate_generator(prs: &[u8], ci: &[u8], sid: &[u8]) -> [u8; 32] {
    let h = sha512(&generator_string(b"CPace255", prs, ci, sid, 128));
    let mut u = [0u8; 32];
    u.copy_from_slice(&h[..32]);
    u[31] &= 0x7f;
    elligator2(&u)
}

/// X25519(y, u); all zeros (G.I) means abort.
pub fn scalar_mult_vfy(y: &[u8; 32], u: &[u8; 32]) -> [u8; 32] {
    curve25519_dalek::montgomery::MontgomeryPoint(*u).mul_clamped(*y).to_bytes()
}

fn neutral(k: &[u8; 32]) -> bool {
    k.iter().all(|b| *b == 0)
}

pub fn isk(sid: &[u8], k: &[u8], ya: &[u8], ada: &[u8], yb: &[u8], adb: &[u8]) -> Vec<u8> {
    sha512(&cat(&[&lv_cat(&[b"CPace255_ISK", sid, k]), &lv_cat(&[ya, ada]), &lv_cat(&[yb, adb])]))
}

fn hmac512(key: &[u8], msg: &[u8]) -> Vec<u8> {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA512, key), msg).as_ref().to_vec()
}

fn channel(fp: &[u8]) -> Vec<u8> {
    lv_cat(&[b"aio-pair-v1", fp])
}

/// What both sides know once K is known.
pub struct Keys {
    pub isk: Vec<u8>,
    pub ta: Vec<u8>,
    pub tb: Vec<u8>,
}

fn keys(sid: &[u8], k: &[u8], ya: &[u8], yb: &[u8], adb: &[u8]) -> Keys {
    let isk = isk(sid, k, ya, b"", yb, adb);
    let mac = sha512(&cat(&[b"CPaceMac", sid, &isk]));
    Keys { ta: hmac512(&mac, &lv_cat(&[ya, b""])), tb: hmac512(&mac, &lv_cat(&[yb, adb])), isk }
}

pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).expect("the OS random source");
    b
}

/// The initiator's state after message 1.
pub struct Start {
    pub sid: [u8; 16],
    y: [u8; 32],
    pub ya: [u8; 32],
}

/// Initiator (the client), message 1.
pub fn start(pin: &str, fp: &[u8], y: [u8; 32], sid: [u8; 16]) -> Start {
    let g = calculate_generator(pin.as_bytes(), &channel(fp), &sid);
    Start { sid, y, ya: scalar_mult_vfy(&y, &g) }
}

/// The responder's answer to message 1; `None` = abort (a low-order Ya).
pub struct Offer {
    pub yb: [u8; 32],
    pub adb: Vec<u8>,
    pub keys: Keys,
}

pub fn respond(pin: &str, fp: &[u8], sid: &[u8], ya: &[u8; 32], y: [u8; 32]) -> Option<Offer> {
    let g = calculate_generator(pin.as_bytes(), &channel(fp), sid);
    let yb = scalar_mult_vfy(&y, &g);
    let k = scalar_mult_vfy(&y, ya);
    (!neutral(&k)).then(|| Offer { yb, adb: fp.to_vec(), keys: keys(sid, &k, ya, &yb, fp) })
}

/// Initiator, on the answer to message 1; `None` = abort.
pub fn finish(st: &Start, yb: &[u8; 32], adb: &[u8]) -> Option<Keys> {
    let k = scalar_mult_vfy(&st.y, yb);
    (!neutral(&k)).then(|| keys(&st.sid, &k, &st.ya, yb, adb))
}

struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn box_key(isk: &[u8], sid: &[u8]) -> aead::LessSafeKey {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, sid).extract(isk);
    let mut k = [0u8; 32];
    prk.expand(&[b"aio-pair-v1 profile"], Len(32)).and_then(|okm| okm.fill(&mut k)).expect("32 bytes from HKDF-SHA256");
    aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, &k).expect("an AES-256 key"))
}

/// iv ‖ AES-256-GCM(profile).
pub fn seal(isk: &[u8], sid: &[u8], plain: &str, iv: [u8; 12]) -> Vec<u8> {
    let mut data = plain.as_bytes().to_vec();
    box_key(isk, sid)
        .seal_in_place_append_tag(aead::Nonce::assume_unique_for_key(iv), aead::Aad::empty(), &mut data)
        .expect("a profile fits AES-GCM");
    cat(&[&iv, &data])
}

/// The profile text; `None` if the box doesn't open under this key.
pub fn open(isk: &[u8], sid: &[u8], sealed: &[u8]) -> Option<String> {
    let (iv, rest) = sealed.split_at_checked(12)?;
    let mut data = rest.to_vec();
    let nonce = aead::Nonce::try_assume_unique_for_key(iv).ok()?;
    let plain = box_key(isk, sid).open_in_place(nonce, aead::Aad::empty(), &mut data).ok()?;
    String::from_utf8(plain.to_vec()).ok()
}

/// SHA-256 of a certificate's DER; empty for none.
pub fn fingerprint(der: &[u8]) -> Vec<u8> {
    if der.is_empty() { vec![] } else { digest::digest(&digest::SHA256, der).as_ref().to_vec() }
}

pub fn b64(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// Base64 (at most 8192 characters), else `None`.
pub fn unb64_vec(s: &serde_json::Value) -> Option<Vec<u8>> {
    use base64::Engine;
    let s = s.as_str().filter(|s| s.len() <= 8192)?;
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

/// Base64 of exactly `N` bytes, else `None`.
pub fn unb64<const N: usize>(s: &serde_json::Value) -> Option<[u8; N]> {
    unb64_vec(s)?.try_into().ok()
}

#[cfg(test)]
mod tests {
    //! The vectors of aio's `tests/cpace-vectors.ts`.
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }
    fn h32(s: &str) -> [u8; 32] {
        hex(s).try_into().unwrap()
    }
    fn to_hex(b: &[u8]) -> String {
        b.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The edges (0..64; 2^255 − 64 .. 2^255 − 1, which holds p and the inputs
    /// past it) and 10 000 pseudo-random inputs (a SHA-512 chain).
    fn inputs() -> Vec<[u8; 32]> {
        let mut out: Vec<[u8; 32]> = (0..64).map(|k| std::array::from_fn(|i| if i == 0 { k } else { 0 })).collect();
        out.extend((0..64).map(|k| std::array::from_fn(|i| [0xc0 + k, 0xff, 0x7f][usize::from(i > 0) + usize::from(i == 31)])));
        let mut h = sha512(b"aio-cpace elligator2");
        for _ in 0..10_000 {
            let mut r: [u8; 32] = h[..32].try_into().unwrap();
            r[31] &= 0x7f;
            out.push(r);
            h = sha512(&h);
        }
        out
    }

    #[test]
    fn elligator2_is_aios() {
        // SHA-256 of the old big-integer elligator2 (num-bigint, aio's math) on
        // every input, in order; it matched this one input by input on 1 010 128
        // inputs (these and a million random ones) before num-bigint went.
        let all: Vec<u8> = inputs().iter().flat_map(elligator2).collect();
        assert_eq!(to_hex(digest::digest(&digest::SHA256, &all).as_ref()), "0726eb0a68eb27430d8842355b104d6eabcddbf6e88eb6f3a08c899f0ba59dd7");
    }

    const CI: &str = "0b415f696e69746961746f720b425f726573706f6e646572";
    const SID: &str = "7e4b4791d6a8ef019b936c79fb7f2c57";
    const YA: &str = "21b4f4bd9e64ed355c3eb676a28ebedaf6d8f17bdc365995b319097153044080";
    const YB: &str = "848b0779ff415f0af4ea14df9dd1d3c29ac41d836c7808896c4eba19c51ac40a";

    #[test]
    fn lengths_are_leb128() {
        assert_eq!(
            to_hex(&lv_cat(&[b"1234", b"5", b"", b"PartyA", b"234", b"5", b"PartyB"])),
            "04313233340135000650617274794103323334013506506172747942"
        );
        assert_eq!(prepend_len(&[0; 200])[..2], [0xc8, 0x01]);
    }

    #[test]
    fn draft_21_appendix_b1() {
        let (ci, sid) = (hex(CI), hex(SID));
        let gen_str = concat!(
            "0843506163653235350850617373776f72646d000000000000000000",
            "00000000000000000000000000000000000000000000000000000000",
            "00000000000000000000000000000000000000000000000000000000",
            "00000000000000000000000000000000000000000000000000000000",
            "00000000000000000000000000000000180b415f696e69746961746f",
            "720b425f726573706f6e646572107e4b4791d6a8ef019b936c79fb7f",
            "2c57"
        );
        assert_eq!(to_hex(&generator_string(b"CPace255", b"Password", &ci, &sid, 128)), gen_str);
        let g = calculate_generator(b"Password", &ci, &sid);
        assert_eq!(to_hex(&g), "d04bf6d41f6a289632a2e929fa29bebd51092512a7829fdde7d314b62f05a73f");
        let ya = scalar_mult_vfy(&h32(YA), &g);
        let yb = scalar_mult_vfy(&h32(YB), &g);
        assert_eq!(to_hex(&ya), "1d13c89278cdadd826f6d8d7f887701430f8380ddc17611cdd6dc989ce0c9f32");
        assert_eq!(to_hex(&yb), "248cccf6d5cdc3646f0ad593f9e6cef4e69d4945f8372e623512ecea32185623");
        let k = scalar_mult_vfy(&h32(YA), &yb);
        assert_eq!(to_hex(&k), "5b067effbdc0b2a0e1d907b21ebb25cfedb96a852179a847c37e43ee71322c6b");
        assert_eq!(scalar_mult_vfy(&h32(YB), &ya), k);
        let isk_ir = concat!(
            "6e19b875f7a561d6b3ca3dbb9ef42ac55de3e717881018204b8922b4",
            "d5e53bb2aa82c300bea7b65d2b671da71922ddf6472301b79bc270ad",
            "fa8bf413285f2263"
        );
        assert_eq!(to_hex(&isk(&sid, &k, &ya, b"ADa", &yb, b"ADb")), isk_ir);
    }

    #[test]
    fn low_order_points_abort() {
        let s = h32("af46e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449aff");
        let zero = [
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0100000000000000000000000000000000000000000000000000000000000000",
            "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
            "e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800",
            "5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157",
            "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
            "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        ];
        for u in zero {
            assert!(neutral(&scalar_mult_vfy(&s, &h32(u))), "{u}");
            assert!(respond("12345678", b"", &hex(SID), &h32(u), random()).is_none(), "{u}");
        }
        let nonzero = [
            (
                "daffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "d8e2c776bbacd510d09fd9278b7edcd25fc5ae9adfba3b6e040e8d3b71b21806",
            ),
            (
                "dbffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "c85c655ebe8be44ba9c0ffde69f2fe10194458d137f09bbff725ce58803cdb38",
            ),
            (
                "d9ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "db64dafa9b8fdd136914e61461935fe92aa372cb056314e1231bc4ec12417456",
            ),
            (
                "cdeb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b880",
                "e062dcd5376d58297be2618c7498f55baa07d7e03184e8aada20bca28888bf7a",
            ),
            (
                "4c9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f11d7",
                "993c6ad11c4c29da9a56f7691fd0ff8d732e49de6250b6c2e80003ff4629a175",
            ),
        ];
        for (u, q) in nonzero {
            assert_eq!(to_hex(&scalar_mult_vfy(&s, &h32(u))), q, "{u}");
        }
    }

    #[test]
    fn aio_golden_vector() {
        let fp = fingerprint(b"aio-pair test leaf");
        assert_eq!(to_hex(&fp), "325c659121f6545ed9e44b6f2a6a0472bb493ba7a33492a76cb48c3e6890c3c6");
        let sid: [u8; 16] = hex(SID).try_into().unwrap();
        let st = start("12345678", &fp, h32(YA), sid);
        assert_eq!(to_hex(&st.ya), "5ec9ee607dcb0dfd7259bda9b970a9d4fe503090ee9a16d38b7e4ee48a6afc4b");
        let r = respond("12345678", &fp, &sid, &st.ya, h32(YB)).unwrap();
        assert_eq!(to_hex(&r.yb), "d8f6867fce3f0338b7361392e2025669c16fcb8e159630fddacdf4b0efc9f143");
        assert_eq!(r.adb, fp);
        let ta = "ec302b0f0ed40d61da78db62a6cc9ae95ee04023653140ae79b14d041f6e13835ed4f9f027e302eac5bf62710294a35c91ae40604ebb51e4f188404f522cd72b";
        let tb = "ba70b91a7794b7d0ded73401ef4edbb238d1c9921eb30f1bafa65f5d559fbb0993dca8ae4e28b870c89e8d74d282cb9c73025d515c761b01dfce9afd5d3c2683";
        assert_eq!((to_hex(&r.keys.ta), to_hex(&r.keys.tb)), (ta.into(), tb.into()));
        let f = finish(&st, &r.yb, &r.adb).unwrap();
        assert_eq!((to_hex(&f.ta), to_hex(&f.tb)), (ta.into(), tb.into()));
        let iv: [u8; 12] = hex("000102030405060708090a0b").try_into().unwrap();
        let mut sealed = seal(&r.keys.isk, &sid, r#"{"aio":1}"#, iv);
        assert_eq!(to_hex(&sealed), "000102030405060708090a0b250d66683eb7167e365b72df0baa2be26ef48987f31b624b95");
        assert_eq!(open(&f.isk, &sid, &sealed).as_deref(), Some(r#"{"aio":1}"#));
        *sealed.last_mut().unwrap() ^= 1;
        assert_eq!(open(&f.isk, &sid, &sealed), None);
    }

    #[test]
    fn a_wrong_pin_or_certificate_gives_other_tags() {
        let sid: [u8; 16] = random();
        let (fp, relay) = (fingerprint(b"aio-pair test leaf"), fingerprint(b"a relay's leaf"));
        let st = start("12345678", &fp, random(), sid);
        for r in [respond("12345679", &fp, &sid, &st.ya, random()), respond("12345678", &relay, &sid, &st.ya, random())]
        {
            let r = r.unwrap();
            assert_ne!(finish(&st, &r.yb, &r.adb).unwrap().ta, r.keys.ta);
        }
    }
}

//! SHA-256, the same bits on every target: ring's natively (aio has ring
//! anyway), the `sha2` crate in a browser, where ring doesn't build.

#[cfg(not(target_arch = "wasm32"))]
type Inner = ring::digest::Context;
#[cfg(target_arch = "wasm32")]
type Inner = sha2::Sha256;

/// A SHA-256 fed in parts.
#[derive(Clone)]
pub struct Sha256(Inner);

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        return Sha256(ring::digest::Context::new(&ring::digest::SHA256));
        #[cfg(target_arch = "wasm32")]
        return Sha256(<sha2::Sha256 as sha2::Digest>::new());
    }

    pub fn update(&mut self, b: impl AsRef<[u8]>) {
        #[cfg(not(target_arch = "wasm32"))]
        self.0.update(b.as_ref());
        #[cfg(target_arch = "wasm32")]
        sha2::Digest::update(&mut self.0, b.as_ref());
    }

    pub fn finalize(self) -> [u8; 32] {
        #[cfg(not(target_arch = "wasm32"))]
        return self.0.finish().as_ref().try_into().expect("SHA-256 is 32 bytes");
        #[cfg(target_arch = "wasm32")]
        return sha2::Digest::finalize(self.0).into();
    }

    /// The digest of `b` in one go.
    pub fn digest(b: impl AsRef<[u8]>) -> [u8; 32] {
        let mut h = Self::new();
        h.update(b);
        h.finalize()
    }
}

/// A file or stream hashed as it's copied in (`io::copy`).
impl std::io::Write for Sha256 {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.update(b);
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `b` in lowercase hex, as `sha256sum` prints a digest.
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_digests_whole_or_in_parts() {
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(hex(&Sha256::digest(b"abc")), abc);
        let mut h = Sha256::new();
        h.update(b"a");
        h.update("bc");
        assert_eq!(hex(&h.finalize()), abc);
        assert_eq!(hex(&Sha256::digest(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}

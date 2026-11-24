//! Android: each secret a file in `data/secrets/`, sealed with AES-GCM by
//! a key the Keystore made and never hands out (in the phone's secure
//! hardware where it has one). A biometric secret's key opens only after
//! the user's fingerprint or face, asked each time (`BiometricPrompt`);
//! enrolling another invalidates it. The Java side is aio-shell's host
//! (`aio.rt.Host.seal`, `open`), handed in as [`crate::KEYSTORE`].
use super::{Files, Secret};
use std::io;

pub(super) struct Os(Files);

/// The Keystore's mark in the file.
const KEYSTORE: u8 = b'k';

fn keystore() -> io::Result<&'static crate::Keystore> {
    crate::KEYSTORE
        .get()
        .ok_or_else(|| io::Error::other("no Keystore yet: the app's Android host hands it in at start"))
}

impl Os {
    pub(super) fn new(_: &str, data: &std::path::Path) -> Self {
        Os(Files::of(data))
    }

    pub(super) fn put(&self, name: &str, v: &[u8], biometric: bool) -> io::Result<()> {
        let sealed = (keystore()?.seal)(biometric, v)?;
        self.0.write(name, KEYSTORE, biometric, &sealed)
    }

    pub(super) fn get(&self, name: &str) -> io::Result<Option<Secret>> {
        let Some((biometric, sealed)) = self.0.read(name, KEYSTORE)? else { return Ok(None) };
        (keystore()?.open)(biometric, &sealed).map(|v| Some(Secret(v)))
    }

    pub(super) fn delete(&self, name: &str) -> io::Result<()> {
        self.0.remove(name)
    }
}

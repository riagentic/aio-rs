//! Windows: each secret a file in `data/secrets/`, sealed by DPAPI
//! (`CryptProtectData`) for this user on this machine, the app's id mixed
//! in: another user, another machine or another app's call can't open it.
//! Files rather than the Credential Manager: no size cap (2.5 KB there),
//! and they go with the app's data when it is deleted.
use super::{Files, Secret, no_biometric, unsupported};
use std::io;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

pub(super) struct Os {
    files: Files,
    id: String,
}

/// DPAPI's mark in the file.
const DPAPI: u8 = b'd';

fn blob(b: &[u8]) -> io::Result<CRYPT_INTEGER_BLOB> {
    let n =
        u32::try_from(b.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a secret of 4 GB or more"))?;
    Ok(CRYPT_INTEGER_BLOB { cbData: n, pbData: b.as_ptr().cast_mut() })
}

/// DPAPI's `f` (protect or unprotect) of `b` with the app's id as entropy:
/// its output copied out, wiped and freed.
fn dpapi(
    id: &str,
    b: &[u8],
    f: unsafe extern "system" fn(
        *const CRYPT_INTEGER_BLOB,
        *const CRYPT_INTEGER_BLOB,
        *mut CRYPT_INTEGER_BLOB,
    ) -> windows_sys::core::BOOL,
) -> io::Result<Secret> {
    let (input, entropy) = (blob(b)?, blob(id.as_bytes())?);
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: the blobs point at live slices DPAPI only reads; it fills `out`.
    if unsafe { f(&input, &entropy, &mut out) } == 0 {
        let e = io::Error::last_os_error();
        return Err(match e.raw_os_error() {
            // A logon without the user's password (a service, an SSH key's) has no DPAPI key.
            Some(5) => unsupported(format!(
                "DPAPI can't seal for this logon ({e}: a service's or an SSH key's, without the user's \
                 password): ask the user each time"
            )),
            _ => e,
        });
    }
    if out.pbData.is_null() {
        return Ok(Secret(vec![]));
    }
    // SAFETY: on success `out` is `cbData` bytes DPAPI allocated with LocalAlloc: read, wiped,
    // freed once.
    unsafe {
        let s = std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize);
        let copy = Secret(s.to_vec());
        for x in s.iter_mut() {
            std::ptr::write_volatile(x, 0);
        }
        LocalFree(out.pbData.cast());
        Ok(copy)
    }
}

unsafe extern "system" fn protect(
    i: *const CRYPT_INTEGER_BLOB,
    e: *const CRYPT_INTEGER_BLOB,
    o: *mut CRYPT_INTEGER_BLOB,
) -> windows_sys::core::BOOL {
    // SAFETY: the caller's blobs, as `dpapi` hands them; no UI, no prompt.
    unsafe {
        CryptProtectData(i, std::ptr::null(), e, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, o)
    }
}

unsafe extern "system" fn unprotect(
    i: *const CRYPT_INTEGER_BLOB,
    e: *const CRYPT_INTEGER_BLOB,
    o: *mut CRYPT_INTEGER_BLOB,
) -> windows_sys::core::BOOL {
    // SAFETY: the caller's blobs, as `dpapi` hands them; no description asked back, no UI.
    unsafe {
        CryptUnprotectData(i, std::ptr::null_mut(), e, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, o)
    }
}

impl Os {
    pub(super) fn new(id: &str, data: &std::path::Path) -> Self {
        Os { files: Files::of(data), id: id.into() }
    }

    pub(super) fn put(&self, name: &str, v: &[u8], biometric: bool) -> io::Result<()> {
        if biometric {
            return Err(no_biometric());
        }
        self.files.write(name, DPAPI, false, &dpapi(&self.id, v, protect)?)
    }

    pub(super) fn get(&self, name: &str) -> io::Result<Option<Secret>> {
        let Some((_, sealed)) = self.files.read(name, DPAPI)? else { return Ok(None) };
        dpapi(&self.id, &sealed, unprotect).map(Some).map_err(|e| match e.kind() {
            io::ErrorKind::Unsupported => e,
            _ => io::Error::new(
                io::ErrorKind::InvalidData,
                format!("secret {name} can't be opened by this user on this machine ({e}): set it again"),
            ),
        })
    }

    pub(super) fn delete(&self, name: &str) -> io::Result<()> {
        self.files.remove(name)
    }
}

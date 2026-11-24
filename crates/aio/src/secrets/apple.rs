//! macOS: the Keychain (Security.framework), a generic password per
//! secret: its service `aio-rs:<app id>:<home>`, its account the secret's
//! name, its label `<app id>: <name>` (what Keychain Access shows). The
//! app that made an item opens it freely; another is asked about.
use super::{Secret, no_biometric, unsupported};
use std::ffi::{c_char, c_void};
use std::io;

type Ref = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeDictionaryKeyCallBacks: [usize; 6];
    static kCFTypeDictionaryValueCallBacks: [usize; 5];
    static kCFBooleanTrue: Ref;
    static kCFAllocatorNull: Ref;
    fn CFDictionaryCreate(
        a: Ref,
        keys: *const Ref,
        values: *const Ref,
        n: isize,
        k: *const c_void,
        v: *const c_void,
    ) -> Ref;
    fn CFStringCreateWithBytes(a: Ref, bytes: *const u8, n: isize, encoding: u32, external: u8) -> Ref;
    fn CFDataCreateWithBytesNoCopy(a: Ref, bytes: *const u8, n: isize, dealloc: Ref) -> Ref;
    fn CFDataGetBytePtr(d: Ref) -> *const u8;
    fn CFDataGetLength(d: Ref) -> isize;
    fn CFStringGetCString(s: Ref, buf: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFRelease(r: Ref);
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecClass: Ref;
    static kSecClassGenericPassword: Ref;
    static kSecAttrService: Ref;
    static kSecAttrAccount: Ref;
    static kSecAttrLabel: Ref;
    static kSecValueData: Ref;
    static kSecReturnData: Ref;
    static kSecMatchLimit: Ref;
    static kSecMatchLimitOne: Ref;
    fn SecItemAdd(attributes: Ref, result: *mut Ref) -> i32;
    fn SecItemCopyMatching(query: Ref, result: *mut Ref) -> i32;
    fn SecItemUpdate(query: Ref, attributes: Ref) -> i32;
    fn SecItemDelete(query: Ref) -> i32;
    fn SecCopyErrorMessageString(status: i32, reserved: *const c_void) -> Ref;
}

const UTF8: u32 = 0x0800_0100;
const NOT_FOUND: i32 = -25300;
const USER_CANCELED: i32 = -128;
const AUTH_FAILED: i32 = -25293;
const INTERACTION_NOT_ALLOWED: i32 = -25308;
const NO_SUCH_KEYCHAIN: i32 = -25294;
const NOT_AVAILABLE: i32 = -25291;

/// A Core Foundation object this code owns: released when dropped.
struct Cf(Ref);

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: an object this code created or was handed with a +1 count, released once.
            unsafe { CFRelease(self.0) }
        }
    }
}

fn string(s: &str) -> Cf {
    // SAFETY: `s`'s bytes, copied by Core Foundation.
    Cf(unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, 0) })
}

fn dict(pairs: &[(Ref, Ref)]) -> Cf {
    let (k, v): (Vec<Ref>, Vec<Ref>) = pairs.iter().copied().unzip();
    // SAFETY: two arrays of `n` live CF objects, retained by the dictionary (the CF type callbacks).
    Cf(unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            k.as_ptr(),
            v.as_ptr(),
            k.len() as isize,
            (&raw const kCFTypeDictionaryKeyCallBacks).cast(),
            (&raw const kCFTypeDictionaryValueCallBacks).cast(),
        )
    })
}

/// What `status` means, in the Keychain's words and as an error kind.
fn failed(status: i32, what: &str) -> io::Error {
    // SAFETY: the error's text, owned here (Cf releases it); copied out as UTF-8.
    let said = unsafe {
        let s = Cf(SecCopyErrorMessageString(status, std::ptr::null()));
        let mut buf = [0 as c_char; 256];
        match !s.0.is_null() && CFStringGetCString(s.0, buf.as_mut_ptr(), buf.len() as isize, UTF8) != 0 {
            true => std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned(),
            false => String::new(),
        }
    };
    let said = format!("the Keychain failed {what}: {said} (OSStatus {status})");
    match status {
        USER_CANCELED | AUTH_FAILED => io::Error::new(io::ErrorKind::PermissionDenied, said),
        // Locked, and no one to ask (an SSH session, a daemon); or no keychain at all.
        INTERACTION_NOT_ALLOWED | NO_SUCH_KEYCHAIN | NOT_AVAILABLE => {
            unsupported(format!("{said}: unlock it, or ask the user each time"))
        }
        _ => io::Error::other(said),
    }
}

pub(super) struct Os {
    service: String,
    id: String,
}

impl Os {
    pub(super) fn new(id: &str, data: &std::path::Path) -> Self {
        Os { service: format!("aio-rs:{id}:{}", super::home_of(data)), id: id.into() }
    }

    pub(super) fn put(&self, name: &str, v: &[u8], biometric: bool) -> io::Result<()> {
        if biometric {
            return Err(no_biometric());
        }
        let (service, account, label) = (string(&self.service), string(name), string(&format!("{}: {name}", self.id)));
        // SAFETY: `v` outlives the data object, which neither copies nor frees it (kCFAllocatorNull).
        let data = Cf(unsafe {
            CFDataCreateWithBytesNoCopy(std::ptr::null(), v.as_ptr(), v.len() as isize, kCFAllocatorNull)
        });
        // SAFETY: the Security constants are live CF strings; every object passed lives through the calls.
        unsafe {
            let query = dict(&[
                (kSecClass, kSecClassGenericPassword),
                (kSecAttrService, service.0),
                (kSecAttrAccount, account.0),
            ]);
            match SecItemUpdate(query.0, dict(&[(kSecValueData, data.0)]).0) {
                0 => Ok(()),
                NOT_FOUND => {
                    let item = dict(&[
                        (kSecClass, kSecClassGenericPassword),
                        (kSecAttrService, service.0),
                        (kSecAttrAccount, account.0),
                        (kSecAttrLabel, label.0),
                        (kSecValueData, data.0),
                    ]);
                    match SecItemAdd(item.0, std::ptr::null_mut()) {
                        0 => Ok(()),
                        s => Err(failed(s, "keeping it")),
                    }
                }
                s => Err(failed(s, "keeping it")),
            }
        }
    }

    pub(super) fn get(&self, name: &str) -> io::Result<Option<Secret>> {
        let (service, account) = (string(&self.service), string(name));
        // SAFETY: as in `put`; a found item's data is handed over with a +1 count (Cf releases it).
        unsafe {
            let query = dict(&[
                (kSecClass, kSecClassGenericPassword),
                (kSecAttrService, service.0),
                (kSecAttrAccount, account.0),
                (kSecReturnData, kCFBooleanTrue),
                (kSecMatchLimit, kSecMatchLimitOne),
            ]);
            let mut out: Ref = std::ptr::null();
            match SecItemCopyMatching(query.0, &mut out) {
                0 => {
                    let data = Cf(out);
                    let (p, n) = (CFDataGetBytePtr(data.0), CFDataGetLength(data.0) as usize);
                    Ok(Some(Secret(if n == 0 { vec![] } else { std::slice::from_raw_parts(p, n).to_vec() })))
                }
                NOT_FOUND => Ok(None),
                s => Err(failed(s, "reading it")),
            }
        }
    }

    pub(super) fn delete(&self, name: &str) -> io::Result<()> {
        let (service, account) = (string(&self.service), string(name));
        // SAFETY: as in `put`.
        unsafe {
            let query = dict(&[
                (kSecClass, kSecClassGenericPassword),
                (kSecAttrService, service.0),
                (kSecAttrAccount, account.0),
            ]);
            match SecItemDelete(query.0) {
                0 | NOT_FOUND => Ok(()),
                s => Err(failed(s, "deleting it")),
            }
        }
    }
}

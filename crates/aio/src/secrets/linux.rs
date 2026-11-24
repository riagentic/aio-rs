//! Linux (and the BSDs): the Secret Service (`org.freedesktop.secrets`:
//! GNOME Keyring, KWallet, KeePassXC) through libsecret, opened with
//! `dlopen` (no crate, no link-time dependency: Electron needs libsecret,
//! so a desktop with a keyring has it). An item per secret in the default
//! collection, found by the app's id, its home and the secret's name.
//! None running (a server, a bare window manager): `Unsupported`.
use super::{Secret, no_biometric, unsupported};
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::io;
use std::sync::OnceLock;

type P = *mut c_void;
type S = *const c_char;

macro_rules! lib_fns {
    ($($name:ident: fn($($a:ty),*) $(-> $r:ty)?;)*) => {
        #[allow(non_snake_case)]
        struct Lib { $($name: unsafe extern "C" fn($($a),*) $(-> $r)?,)* }
        impl Lib {
            /// SAFETY: `h` is a live `dlopen` handle; each symbol has the C
            /// signature written here.
            unsafe fn load(h: P) -> Result<Self, String> {
                Ok(Self { $($name: {
                    // SAFETY: `h` is a live dlopen handle (`load`'s contract) and the name a NUL-terminated literal.
                    let p = unsafe { libc::dlsym(h, concat!(stringify!($name), "\0").as_ptr().cast()) };
                    if p.is_null() {
                        return Err(format!("libsecret-1.so.0 has no {} (libsecret 0.19 or newer has)", stringify!($name)));
                    }
                    // SAFETY: `p` is the non-null address of the C function named, whose signature is the one written (`load`'s contract); fn pointers and data pointers have one size here.
                    unsafe { std::mem::transmute::<P, unsafe extern "C" fn($($a),*) $(-> $r)?>(p) }
                },)* })
            }
        }
    };
}

// libsecret's, and GLib's and GIO's it links (found through its handle).
lib_fns! {
    secret_password_storev_binary_sync: fn(*const Schema, P, S, S, P, P, *mut *mut GError) -> c_int;
    secret_password_lookupv_binary_sync: fn(*const Schema, P, P, *mut *mut GError) -> P;
    secret_password_clearv_sync: fn(*const Schema, P, P, *mut *mut GError) -> c_int;
    secret_value_new: fn(S, isize, S) -> P;
    secret_value_get: fn(P, *mut usize) -> S;
    secret_value_unref: fn(P);
    g_hash_table_new: fn(P, P) -> P;
    g_hash_table_insert: fn(P, P, P) -> c_int;
    g_hash_table_unref: fn(P);
    g_str_hash: fn(P) -> c_uint;
    g_str_equal: fn(P, P) -> c_int;
    g_error_free: fn(*mut GError);
    g_io_error_quark: fn() -> u32;
    g_dbus_error_quark: fn() -> u32;
}

/// `SecretSchemaAttribute`.
#[repr(C)]
struct Attr {
    name: S,
    kind: c_int,
}

/// `SecretSchema`.
#[repr(C)]
struct Schema {
    name: S,
    flags: c_int,
    attributes: [Attr; 32],
    reserved: c_int,
    reserved_p: [P; 7],
}

/// `GError`.
#[repr(C)]
struct GError {
    domain: u32,
    code: c_int,
    message: S,
}

/// `G_IO_ERROR_CANCELLED`: a prompt the user dismissed.
const CANCELLED: c_int = 19;

fn lib() -> io::Result<&'static Lib> {
    static LIB: OnceLock<Result<Lib, String>> = OnceLock::new();
    LIB.get_or_init(|| {
        // SAFETY: dlopen of a system library by name; never closed (GLib's threads may outlive any call).
        let h = unsafe { libc::dlopen(c"libsecret-1.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if h.is_null() {
            return Err("libsecret is not installed (libsecret-1.so.0): install it with a keyring \
                        (GNOME Keyring, KWallet, KeePassXC), or ask the user each time"
                .into());
        }
        // SAFETY: `h` is the live handle just opened; `lib_fns!` writes each symbol's C signature.
        unsafe { Lib::load(h) }
    })
    .as_ref()
    .map_err(|e| unsupported(e.clone()))
}

pub(super) struct Os {
    id: CString,
    home: CString,
}

impl Os {
    pub(super) fn new(id: &str, data: &std::path::Path) -> Self {
        // An app id and a hex hash hold no NUL.
        Os { id: CString::new(id).unwrap_or_default(), home: CString::new(super::home_of(data)).unwrap_or_default() }
    }

    /// Run `f` with the schema and secret `name`'s attributes; its error, if
    /// any, said as `what` failing.
    fn with<T>(
        &self,
        name: &str,
        what: &str,
        f: impl FnOnce(&Lib, *const Schema, P, *mut *mut GError) -> T,
    ) -> io::Result<T> {
        let l = lib()?;
        let name = CString::new(name).map_err(io::Error::other)?;
        // Strings, the rest of the 32 unused.
        let mut attributes: [Attr; 32] = std::array::from_fn(|_| Attr { name: std::ptr::null(), kind: 0 });
        for (a, n) in attributes.iter_mut().zip([c"app", c"home", c"name"]) {
            a.name = n.as_ptr();
        }
        let schema = Schema {
            name: c"rs.aio.Secret".as_ptr(),
            flags: 0,
            attributes,
            reserved: 0,
            reserved_p: [std::ptr::null_mut(); 7],
        };
        let mut err: *mut GError = std::ptr::null_mut();
        // SAFETY: the table borrows the C strings above, alive until it is unreffed below; GLib's
        // string hash and equality take them as written.
        let out = unsafe {
            let t = (l.g_hash_table_new)(l.g_str_hash as P, l.g_str_equal as P);
            for (k, v) in [(c"app", &self.id), (c"home", &self.home), (c"name", &name)] {
                (l.g_hash_table_insert)(t, k.as_ptr() as P, v.as_ptr() as P);
            }
            let out = f(l, &schema, t, &mut err);
            (l.g_hash_table_unref)(t);
            out
        };
        match err.is_null() {
            true => Ok(out),
            // SAFETY: a non-null GError libsecret set, freed once here.
            false => Err(unsafe { failed(l, err, what) }),
        }
    }

    pub(super) fn put(&self, name: &str, v: &[u8], biometric: bool) -> io::Result<()> {
        if biometric {
            return Err(no_biometric());
        }
        let label = CString::new(format!("{}: {name}", self.id.to_string_lossy())).map_err(io::Error::other)?;
        let stored = self.with(name, "keeping it", |l, schema, attrs, err| {
            // SAFETY: the value copies `v` (into libsecret's locked memory) and is unreffed once; the
            // schema, attributes and strings live through the call.
            unsafe {
                let value =
                    (l.secret_value_new)(v.as_ptr().cast(), v.len() as isize, c"application/octet-stream".as_ptr());
                let ok = (l.secret_password_storev_binary_sync)(
                    schema,
                    attrs,
                    std::ptr::null(),
                    label.as_ptr(),
                    value,
                    std::ptr::null_mut(),
                    err,
                );
                (l.secret_value_unref)(value);
                ok
            }
        })?;
        match stored {
            0 => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("secret {name} was not kept: the keyring stayed locked"),
            )),
            _ => Ok(()),
        }
    }

    pub(super) fn get(&self, name: &str) -> io::Result<Option<Secret>> {
        self.with(name, "reading it", |l, schema, attrs, err| {
            // SAFETY: the schema and attributes live through the call; a value found is read for its
            // length and unreffed once (libsecret wipes it).
            unsafe {
                let value = (l.secret_password_lookupv_binary_sync)(schema, attrs, std::ptr::null_mut(), err);
                if value.is_null() {
                    return None;
                }
                let mut n = 0usize;
                let p = (l.secret_value_get)(value, &mut n);
                let s = Secret(if n == 0 { vec![] } else { std::slice::from_raw_parts(p.cast::<u8>(), n).to_vec() });
                (l.secret_value_unref)(value);
                Some(s)
            }
        })
    }

    pub(super) fn delete(&self, name: &str) -> io::Result<()> {
        // SAFETY: the schema and attributes live through the call.
        self.with(name, "deleting it", |l, schema, attrs, err| unsafe {
            (l.secret_password_clearv_sync)(schema, attrs, std::ptr::null_mut(), err)
        })
        .map(drop)
    }
}

/// `e` as an error, freed: no Secret Service (no session bus, none on it)
/// is `Unsupported`, a dismissed prompt `PermissionDenied`.
///
/// SAFETY: `e` is a live GError, owned by the caller, not used after.
unsafe fn failed(l: &Lib, e: *mut GError, what: &str) -> io::Error {
    // SAFETY: `e` is live (the contract); its message a NUL-terminated string or null.
    let (domain, code, msg) = unsafe {
        let m = (*e).message;
        (
            (*e).domain,
            (*e).code,
            if m.is_null() { String::new() } else { CStr::from_ptr(m).to_string_lossy().into_owned() },
        )
    };
    // SAFETY: freed once, never used after (the contract).
    unsafe { (l.g_error_free)(e) };
    // SAFETY: GIO functions with no arguments.
    let (io_domain, dbus_domain) = unsafe { ((l.g_io_error_quark)(), (l.g_dbus_error_quark)()) };
    match (domain, code) {
        (d, CANCELLED) if d == io_domain => {
            io::Error::new(io::ErrorKind::PermissionDenied, format!("the keyring's prompt was dismissed ({what})"))
        }
        // No session bus (GIO's), or none on it answering for the Secret Service (D-Bus's).
        (d, _) if d == io_domain || d == dbus_domain => unsupported(format!(
            "no Secret Service keyring on this session's D-Bus ({msg}): run one (GNOME Keyring, KWallet, \
             KeePassXC), or ask the user each time"
        )),
        _ => io::Error::other(format!("the Secret Service failed {what}: {msg}")),
    }
}

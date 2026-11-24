//! The OS's secret store, one small API everywhere: a key, a token, a
//! session's secret kept by the OS for this app, never in its files in the
//! clear, its cells or its log (the guide: `docs/auth/keychain.md`).
//!
//! | Where | A secret is kept |
//! |---|---|
//! | macOS | in the Keychain: an item per secret |
//! | Linux | by the Secret Service (GNOME Keyring, KWallet, KeePassXC) over D-Bus, through libsecret loaded at run time |
//! | Windows | in `data/secrets/`, each sealed by DPAPI for this user |
//! | Android | in `data/secrets/`, each sealed (AES-GCM) by a key the Keystore keeps and never hands out |
//! | the web | nowhere: refused |
//! | the test world | in the sandbox, plain: never the OS's store |
//!
//! Each home of the app has its own (a profile, a dev build, `--home`): a
//! throwaway run never sees the real ones. Every call waits on the OS,
//! which may ask the user (a locked keyring, a fingerprint), so each is
//! `async` and runs off the caller's thread.
//!
//! Errors say what happened by their kind:
//! - `Unsupported`: no store here (the web, Linux with no Secret Service
//!   running, a fingerprint asked for off Android or with none enrolled):
//!   do without it (ask the user each time);
//! - `PermissionDenied`: the user said no;
//! - `InvalidData`: the secret can't be opened here any more (another
//!   user's or machine's, a fingerprint enrolled since): set it again;
//! - `InvalidInput`: a name that isn't 1 to 64 of `a-z`, `0-9`, `-`, `_`.
use std::io;

/// A secret's bytes: wiped from memory when dropped, never printed. The
/// OS's own copies are its to wipe.
pub struct Secret(Vec<u8>);

impl std::ops::Deref for Secret {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        for b in self.0.iter_mut() {
            // SAFETY: `b` is a valid, aligned, exclusive `&mut u8`.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

/// Keep `value` as secret `name`, replacing the one of that name.
pub async fn set(name: &str, value: &[u8]) -> io::Result<()> {
    let v = Secret(value.to_vec());
    run(name, move |s, n| s.put(n, &v, false)).await
}

/// [`set`], opened again only after the user's fingerprint or face: each
/// [`get`] of it asks, and so does this. Android only (its Keystore);
/// elsewhere, and with no fingerprint or face enrolled, `Unsupported`.
pub async fn set_biometric(name: &str, value: &[u8]) -> io::Result<()> {
    let v = Secret(value.to_vec());
    run(name, move |s, n| s.put(n, &v, true)).await
}

/// Secret `name`, if one is kept.
pub async fn get(name: &str) -> io::Result<Option<Secret>> {
    run(name, |s, n| s.get(n)).await
}

/// Forget secret `name` (none kept: nothing to do).
pub async fn delete(name: &str) -> io::Result<()> {
    run(name, |s, n| s.delete(n)).await
}

/// Is `name` one a secret may have on every OS (a file name on Windows
/// and Android, one spelling whatever the file system's case)? Pure.
fn check(name: &str) -> io::Result<()> {
    let ok = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_';
    match (1..=64).contains(&name.len()) && name.bytes().all(ok) {
        true => Ok(()),
        false => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("secret name {name:?}: 1 to 64 of a-z, 0-9, - and _"),
        )),
    }
}

fn unsupported(why: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, why.into())
}

/// No fingerprint here (off Android).
#[cfg_attr(any(target_os = "android", target_arch = "wasm32"), allow(dead_code))]
fn no_biometric() -> io::Error {
    unsupported("a fingerprint or face guards a secret on Android only: here use secrets::set")
}

#[cfg(target_arch = "wasm32")]
async fn run<T>(name: &str, _: impl FnOnce(&Store, &str) -> io::Result<T>) -> io::Result<T> {
    check(name)?;
    Err(unsupported("the web has no OS secret store (a page's storage is any script's on it): ask the user each time"))
}

#[cfg(target_arch = "wasm32")]
struct Store;

#[cfg(target_arch = "wasm32")]
impl Store {
    fn put(&self, _: &str, _: &[u8], _: bool) -> io::Result<()> {
        unreachable!("the web refuses first")
    }
    fn get(&self, _: &str) -> io::Result<Option<Secret>> {
        unreachable!("the web refuses first")
    }
    fn delete(&self, _: &str) -> io::Result<()> {
        unreachable!("the web refuses first")
    }
}

#[cfg(target_vendor = "apple")]
#[path = "secrets/apple.rs"]
mod os;
#[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
#[path = "secrets/linux.rs"]
mod os;
#[cfg(windows)]
#[path = "secrets/windows.rs"]
mod os;
#[cfg(target_os = "android")]
#[path = "secrets/android.rs"]
mod os;

/// One call at a time: a set is never half seen by a get, and the user is
/// asked one thing at a time.
#[cfg(not(target_arch = "wasm32"))]
static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(not(target_arch = "wasm32"))]
async fn run<T: Send + 'static>(
    name: &str,
    f: impl FnOnce(&Store, &str) -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    check(name)?;
    let name = name.to_string();
    crate::rt()
        .spawn_blocking(move || {
            let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
            let (dirs, test) = crate::paths::app_dirs_in()?;
            f(&pick(&dirs.id, &dirs.data, test), &name)
        })
        .await
        .map_err(io::Error::other)?
}

/// Where an app's secrets are kept.
#[cfg(not(target_arch = "wasm32"))]
enum Store {
    /// The test world's: plain files in the sandbox.
    Plain(Files),
    /// The OS's.
    Os(os::Os),
}

/// The store of app `id` with data dir `data`, in the test world or not.
#[cfg(not(target_arch = "wasm32"))]
fn pick(id: &str, data: &std::path::Path, test: bool) -> Store {
    match test {
        true => Store::Plain(Files::of(data)),
        false => Store::Os(os::Os::new(id, data)),
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Store {
    fn put(&self, name: &str, v: &[u8], biometric: bool) -> io::Result<()> {
        match self {
            Store::Plain(f) => f.write(name, b'p', biometric, v),
            Store::Os(o) => o.put(name, v, biometric),
        }
    }

    fn get(&self, name: &str) -> io::Result<Option<Secret>> {
        match self {
            Store::Plain(f) => Ok(f.read(name, b'p')?.map(|(_, s)| s)),
            Store::Os(o) => o.get(name),
        }
    }

    fn delete(&self, name: &str) -> io::Result<()> {
        match self {
            Store::Plain(f) => f.remove(name),
            Store::Os(o) => o.delete(name),
        }
    }
}

/// An app home's own key in the OS's store: its data dir's hash (pure).
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(any(windows, target_os = "android"), allow(dead_code))]
fn home_of(data: &std::path::Path) -> String {
    let home = std::path::absolute(data).unwrap_or_else(|_| data.into());
    crate::sha256::hex(&crate::sha256::Sha256::digest(home.as_os_str().as_encoded_bytes())[..8])
}

/// Sealed secrets as files, `data/secrets/s_<name>`: a byte saying who
/// sealed it, one saying whether a fingerprint opens it, then what the
/// sealer made. Left out of backups (sealed for this machine).
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(target_vendor = "apple", allow(dead_code))]
struct Files(std::path::PathBuf);

#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(target_vendor = "apple", allow(dead_code))]
impl Files {
    fn of(data: &std::path::Path) -> Self {
        Files(data.join("secrets"))
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        // Prefixed: a name may be one Windows keeps for a device (`con`).
        self.0.join(format!("s_{name}"))
    }

    /// Keep `body`, sealed by `by`, whole or not at all.
    fn write(&self, name: &str, by: u8, biometric: bool, body: &[u8]) -> io::Result<()> {
        let mut file = Secret(Vec::with_capacity(body.len() + 2));
        file.0.extend_from_slice(&[by, biometric as u8]);
        file.0.extend_from_slice(body);
        crate::paths::private_dir(&self.0)?;
        crate::paths::write_atomic(&self.path(name), &file, true)
    }

    /// Secret `name`'s body, sealed by `by`, and whether a fingerprint opens
    /// it; none kept: `None`.
    fn read(&self, name: &str, by: u8) -> io::Result<Option<(bool, Secret)>> {
        let file = match std::fs::read(self.path(name)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            r => Secret(r?),
        };
        match &file[..] {
            [b, f @ (0 | 1), body @ ..] if *b == by => Ok(Some((*f == 1, Secret(body.to_vec())))),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("secret {name} was not sealed by this device's store: delete it and set it again"),
            )),
        }
    }

    fn remove(&self, name: &str) -> io::Result<()> {
        match std::fs::remove_file(self.path(name)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn block<T>(f: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
    }

    #[test]
    fn a_test_keeps_its_secrets_in_its_own_world_never_the_os_store() {
        let world = crate::paths::sandbox().unwrap();
        assert!(matches!(pick("app", std::path::Path::new("/d"), true), Store::Plain(_)), "the test world's, always");
        block(async {
            assert!(get("session").await.unwrap().is_none());
            set("session", b"k1").await.unwrap();
            set("session", b"k2").await.unwrap();
            assert_eq!(&*get("session").await.unwrap().unwrap(), b"k2", "set replaces");
            set_biometric("pin", b"1234").await.unwrap();
            assert_eq!(&*get("pin").await.unwrap().unwrap(), b"1234", "a test stands in for the fingerprint");
            set("empty", b"").await.unwrap();
            assert_eq!(get("empty").await.unwrap().unwrap().len(), 0);
            delete("session").await.unwrap();
            delete("session").await.unwrap();
            assert!(get("session").await.unwrap().is_none());
        });
        let dir = crate::paths::app_dirs().unwrap().data.join("secrets");
        assert!(dir.starts_with(world.root()), "under the sandbox: {}", dir.display());
        assert_eq!(std::fs::read(dir.join("s_pin")).unwrap(), b"p\x011234");
    }

    #[test]
    fn a_name_is_one_every_os_keeps_alike() {
        for ok in ["a", "session-key", "wallet_2", &"x".repeat(64)] {
            check(ok).unwrap();
        }
        for bad in ["", "Session", "a.b", "a/b", "..", "é", "a b", &"x".repeat(65)] {
            let e = check(bad).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{bad:?}");
            assert!(e.to_string().contains("1 to 64 of a-z, 0-9, - and _"), "{e}");
        }
        let e = block(get("../sync.token")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "refused before any store is asked");
    }

    #[test]
    fn a_secret_is_never_printed() {
        let s = Secret(b"hunter2".to_vec());
        assert_eq!(format!("{s:?}"), "Secret(…)");
        assert_eq!(&*s, b"hunter2");
    }

    #[test]
    fn a_file_sealed_by_another_store_or_cut_short_is_refused_saying_so() {
        let d = tempfile::tempdir().unwrap();
        let f = Files::of(d.path());
        f.write("a", b'd', false, b"sealed").unwrap();
        assert_eq!(f.read("a", b'd').unwrap().map(|(b, s)| (b, s.to_vec())), Some((false, b"sealed".to_vec())));
        for (file, by) in [(&b"dx"[..], b'd'), (b"d", b'd'), (b"", b'd'), (b"d\x00x", b'k')] {
            std::fs::write(f.path("a"), file).unwrap();
            let e = f.read("a", by).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::InvalidData, "{file:?}");
            assert!(e.to_string().contains("set it again"), "{e}");
        }
        f.remove("a").unwrap();
        f.remove("a").unwrap();
        assert!(f.read("a", b'd').unwrap().is_none());
    }

    #[test]
    fn each_home_has_its_own_secrets() {
        let (a, b) = (home_of(std::path::Path::new("/h/a/data")), home_of(std::path::Path::new("/h/b/data")));
        assert_ne!(a, b);
        assert_eq!(a, home_of(std::path::Path::new("/h/a/data")), "the same home, the same key");
        assert_eq!(a.len(), 16);
    }

    /// The OS's own store, for real: a secret kept, replaced, read and
    /// forgotten, under a throwaway home. Where it can't be used (Linux with
    /// no session bus, as `ci.sh` runs; DPAPI or a locked Keychain over SSH)
    /// it says what to do instead, unless `AIO_CI_SECRETS` says it must be
    /// there (GitHub CI; `ci.sh`'s own keyring).
    #[test]
    fn the_os_store_keeps_replaces_and_forgets_a_secret() {
        let d = tempfile::tempdir().unwrap();
        let s = pick("aio-rs-test", d.path(), false);
        let name = format!("t-{}", crate::uuid_v4().simple());
        if std::env::var_os("AIO_CI_SECRETS").is_none()
            && let Err(e) = s.put(&name, b"x", false)
        {
            assert_eq!(e.kind(), io::ErrorKind::Unsupported, "{e}");
            assert!(e.to_string().contains("ask the user each time"), "it says what to do: {e}");
            return;
        }
        s.delete(&name).unwrap();
        assert!(s.get(&name).unwrap().is_none());
        s.put(&name, b"first \x00\xff", false).unwrap();
        s.put(&name, b"second \x00\xff", false).unwrap();
        assert_eq!(&*s.get(&name).unwrap().unwrap(), b"second \x00\xff", "binary, replaced");
        let other = pick("aio-rs-test", &d.path().join("other-home"), false);
        assert!(other.get(&name).unwrap().is_none(), "another home of the app sees none of it");
        let e = s.put(&name, b"x", true).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported, "a fingerprint: Android only");
        s.delete(&name).unwrap();
        s.delete(&name).unwrap();
        assert!(s.get(&name).unwrap().is_none());
    }
}

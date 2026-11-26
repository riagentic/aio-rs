# The OS's secret store

`aio::secrets` (feature `secrets`) keeps a secret the app needs again (a
session key, a provider's token, a refresh token) in the store the OS
keeps for the user, so it is never in the app's files in the clear, its
cells or its log. One API on every platform:

| Call | What it does |
|---|---|
| `secrets::set(name, &bytes)` | keep it, replacing the one of that name |
| `secrets::get(name)` | `Some(Secret)`, or `None` when none is kept |
| `secrets::delete(name)` | forget it (none kept: nothing to do) |
| `secrets::set_biometric(name, &bytes)` | keep it so each `get` asks for the user's fingerprint or face (Android, the web) |

```toml
# the app's Cargo.toml
aio = { package = "aio-rs", git = "https://github.com/riagentic/aio-rs", tag = "v0.5.0", features = ["kit", "secrets"] }
```

Each call is `async`: the OS may ask the user (a locked keyring, a
fingerprint, a passkey), so it runs off the caller's thread. Call it from an effect,
an async method's awaited future, a handler's task or `main`, never from
a method's body (a method has no I/O, [methods](../state/methods.md)).

A `Secret` derefs to `&[u8]`, prints as `Secret(…)` and wipes its bytes
when dropped. A name is 1 to 64 of `a-z`, `0-9`, `-` and `_`: one
spelling every OS keeps alike.

## Where each platform keeps it

| Platform | Where | Who can read it |
|---|---|---|
| macOS | the Keychain: a generic password per secret, service `aio-rs:<app id>:<home>`, account the name, labelled `<app id>: <name>` in Keychain Access | the app that made it; another app is asked about by the OS |
| Linux | the Secret Service (`org.freedesktop.secrets`: GNOME Keyring, KWallet, KeePassXC), through libsecret loaded at run time: an item per secret in the default keyring, found by `app`, `home` and `name` | the user's session while the keyring is unlocked |
| Windows | `data/secrets/`, each file sealed by DPAPI for this user on this machine, the app's id mixed in | this user, on this machine |
| Android | `data/secrets/`, each file sealed with AES-GCM by a key the Keystore made and never hands out (the phone's secure hardware where it has one) | this app, on this phone; a `set_biometric` one only after the fingerprint or face |
| the web | the page's `localStorage`, each secret sealed with AES-GCM by a key derived from the app's passkey (WebAuthn's PRF): [the web](#the-web-a-passkey) | the page, after the user verified the passkey (fingerprint, face, PIN, security key) |
| a test | plain files in the test's sandbox, never the OS's store | the test |

- Each home of the app has its own secrets: a `--profile`, a debug build's
  `dev` home, a `--home` or `AIORT_DATA` run never sees the real ones (the
  home is part of the key).
- `data/secrets/` is left out of backups and kept as it is by a restore:
  what is sealed for this machine opens on no other, as with `sync.token`.
- macOS knows an app by its signature: a signed app keeps its items
  through updates; an unsigned build is another app after each rebuild,
  so the Keychain asks the user once ("Always Allow").
- Deleting the app's data deletes the Windows and Android secrets with it.
  The Keychain's and the Secret Service's stay until `delete`d: forget
  what you no longer need.
- The OS's own copies (the Keychain's, libsecret's, Java's) are the OS's
  to wipe; aio-rs wipes its own.

## When there is no store

The kind of the `io::Error` says what happened, so the app can carry on
without remembering:

| Kind | When | What the app does |
|---|---|---|
| `Unsupported` | no store here: a browser or passkey without PRF, a page not on https (or `localhost`) or on an IP, a worker; Linux with no session bus or no Secret Service on it (a server, a bare window manager); a locked Keychain with no one to ask (an SSH session); DPAPI in a logon without the user's password (a service, an SSH key's); `set_biometric` off Android and the web, or with no fingerprint or face enrolled | ask the user each time; offer no "remember me" |
| `PermissionDenied` | the user said no: a cancelled fingerprint, keyring or passkey prompt, a passkey not verified; on the web also a passkey no longer on this device (the browser can't tell the page which) | ask again later, or the other way |
| `InvalidData` | it can't be opened here any more: another user's or machine's file, a fingerprint enrolled since (Android invalidates the key), a passkey made since or a stored copy changed (the web) | `delete` it and ask again |
| `InvalidInput` | the name is not 1 to 64 of `a-z 0-9 - _` | fix the name |

Nothing falls back to a plain file: a store that isn't there is said, never
pretended. On the web nothing falls back to the page's storage in the
clear, or to a key kept beside what it seals.

## Remembering a session key

The secret stays out of cells and the log ([secrets and
observability](secrets.md)): it is read and kept in the app's own code.

```rust
use aio::secrets;
use std::io::ErrorKind;

/// The key from the passphrase (a stand-in: use Argon2 or scrypt).
fn derive(passphrase: &str) -> Vec<u8> {
    aio::sha256::Sha256::digest(passphrase.as_bytes()).to_vec()
}

/// The session key: the one this device keeps, else the passphrase's,
/// kept from now on where the device has a store.
async fn session_key(passphrase: impl FnOnce() -> String) -> std::io::Result<Vec<u8>> {
    if let Some(kept) = secrets::get("session").await? {
        return Ok(kept.to_vec());
    }
    let key = derive(&passphrase());
    match secrets::set("session", &key).await {
        Err(e) if e.kind() == ErrorKind::Unsupported => eprintln!("not remembered here: {e}"),
        kept => kept?,
    }
    Ok(key)
}

/// Sign out: the next start asks again.
async fn forget() -> std::io::Result<()> {
    secrets::delete("session").await
}

# fn main() {
# let _world = aio::paths::sandbox().unwrap();
# aio::rt().block_on(async {
let first = session_key(|| "correct horse battery staple".into()).await.unwrap();
let again = session_key(|| unreachable!("kept: not asked")).await.unwrap();
assert_eq!(first, again);
forget().await.unwrap();
assert!(secrets::get("session").await.unwrap().is_none());
# });
# }
```

## A fingerprint or face (Android)

`set_biometric` keeps the secret under a second Keystore key that opens
only after the user's strong biometric, asked by the system's prompt each
time it is read (and when it is kept). The APK must declare it:

```toml
[build.android]
biometric = true
```

Without it, with no fingerprint or face enrolled, or off Android, the call
is `Unsupported`; a dismissed prompt or too many tries is
`PermissionDenied`. Enrolling another fingerprint invalidates the key: the
next `get` is `InvalidData`, and the next `set_biometric` makes a new key.
A plain `set` never asks.

## The web: a passkey

In a page (`ax build --web`) the browser has no store of the OS's, so the
app's passkey derives the key, with web standards only:

- The first `set` asks the browser to make the app's passkey ("save a
  passkey" for the page, named `<page title>: secrets kept in this
  browser`). Tell the user why before that call: the browser's prompt is
  all they see. One passkey per origin: its id is kept beside the secrets;
  made again (its id gone), it replaces the old one on the same device.
- Each `set` and `get` asks for the passkey with user verification
  required (fingerprint, face, PIN or security key) and its PRF
  (WebAuthn's `prf` extension, on CTAP's `hmac-secret`): 32 bytes only that
  passkey gives, only to pages of its host. HKDF-SHA-256 of them (salt: the
  page's origin; info: the secret's name) is the secret's AES-GCM key, a
  non-extractable WebCrypto key that lives while the call runs. Nothing
  derived is kept.
- `localStorage` keeps `aio-secrets/s_<name>`, the envelope (base64 of a
  scheme byte, 1 today, the 12-byte nonce, then the ciphertext and tag),
  and `aio-secrets/passkey`, the passkey's id. `localStorage`, not
  IndexedDB: the page keeps its cells there, a write is whole, and both are
  the origin's alike (any script on it reads them), so only ciphertext
  goes there.
- A `get` of a name never kept and a `delete` ask nothing; `delete` leaves
  the passkey for the next `set`.
- `set_biometric` is `set` here: PRF verifies the user each time anyway
  (`hmac-secret` gives another output without verification, so a key
  would change with it: always required).
- The origin is the app's home: two apps served from one origin share its
  passkey and secrets, as they share all else the browser keeps for it.
  Serve each from its own origin.

Where it works: a page on `https` (or `http://localhost`) on a domain (not
an IP), in a browser and passkey provider with PRF: current Chrome, Edge,
Safari and Firefox with Google Password Manager, iCloud Keychain, or a
FIDO2 security key with `hmac-secret`. Elsewhere (Windows Hello and
other providers without PRF) the first `set` is `Unsupported` saying so:
the passkey made in vain is not kept, and the browser is asked to forget it
(`signalUnknownCredential`, where it has that). Where the browser says
up front it has no PRF (`getClientCapabilities`), no passkey is made.

What it protects, and what it doesn't:

| Threat | |
|---|---|
| a copy of the browser's storage (a stolen profile, a backup, a sync, a disk image) | protected: ciphertext, and the key needs the passkey and the user |
| another origin's page | protected: the ciphertext is in this origin's storage only. The passkey (its RP ID the page's host) also answers pages on that host's other ports and its subdomains, with the user's verification: serve nothing untrusted there |
| a changed or swapped envelope | refused (`InvalidData`): AES-GCM's tag, and each name its own key |
| a script running in the page (XSS, a compromised dependency) | not protected: it can call `get` (the user sees a passkey prompt they may accept) or read what the app holds. Keep the page's content security policy strict, as for any secret in a page |
| the passkey's provider or device | trusted: whoever holds the passkey and passes its verification derives the key |

The passkey signs a random challenge no one checks: it proves nothing to
a server and is not a sign-in (that is `aio::auth`'s). The PRF's output,
copied into the module's memory for the HKDF, is wiped there when the
call ends; the browser's own copies are its to drop.

## In tests

In the test world (`TestCells`, `TestUi`, `aio::paths::sandbox()`) the
secrets are plain files in the sandbox's app home, gone with it, and the
OS's store is never touched: tests of code that keeps secrets just run.
`set_biometric` stands in for the fingerprint there (it always opens), so
a test can't see a cancelled prompt: give that path its own function to
test.

## API

`cargo doc -p aio-rs --no-deps --features secrets --open`: `aio::secrets`
(`set`, `set_biometric`, `get`, `delete`, `Secret`).

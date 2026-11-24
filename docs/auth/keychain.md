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
| `secrets::set_biometric(name, &bytes)` | keep it so each `get` asks for the user's fingerprint or face (Android) |

```toml
# the app's Cargo.toml
aio = { package = "aio-rs", version = "0.4", features = ["kit", "secrets"] }
```

Each call is `async`: the OS may ask the user (a locked keyring, a
fingerprint), so it runs off the caller's thread. Call it from an effect,
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
| the web | nowhere: every call is `Unsupported` | |
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
| `Unsupported` | no store here: the web; Linux with no session bus or no Secret Service on it (a server, a bare window manager); a locked Keychain with no one to ask (an SSH session); DPAPI in a logon without the user's password (a service, an SSH key's); `set_biometric` off Android, or with no fingerprint or face enrolled | ask the user each time; offer no "remember me" |
| `PermissionDenied` | the user said no: a cancelled fingerprint or keyring prompt | ask again later, or the other way |
| `InvalidData` | it can't be opened here any more: another user's or machine's file, a fingerprint enrolled since (Android invalidates the key) | `delete` it and ask again |
| `InvalidInput` | the name is not 1 to 64 of `a-z 0-9 - _` | fix the name |

Nothing falls back to a plain file: a store that isn't there is said, never
pretended. On the web the page's storage is any script's on it; what is
possible there (a WebAuthn passkey's PRF to derive a key with the user's
presence) is not part of this API.

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

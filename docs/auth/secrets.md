# Secrets and observability

Everything that goes through a method is recorded, and a shared cell's
state goes to every client: that is what makes an aio-rs app replayable
and synced. For a secret (a key, a seed, a password, a provider's token)
it is exactly what to reason about. This page says what leaves the
server, what is recorded where, and the patterns that keep a secret out
of both.

## Where a value goes

| Channel | What flows | Who sees it | Keeps a field out |
|---|---|---|---|
| **the wire** (sync) | a shared cell's state, filtered per viewer | every connected client | `#[server]` on the field, `#[cell(visible = f)]` |
| **the event log** | every call: method, **arguments**, caller, time | replay, time travel, `ax events`, `ax record`, synced history | `#[redact]` on the method |
| **the saved state** | the cell's state, in snapshots and the store | backups, `ax snapshot`, problem reports, `ax state` | `#[transient]` on the field |
| **logs** | what the app and aio-rs write | `logs/*.log`, `ax logs` | masking (below), and not logging it |
| **problem reports** | the saved state, the newest calls (never their arguments), diagnostics | `data/reports/`, and `[feedback] url` if set | a `#[redact]` method withholds the cell's state |

A value is safe only when it is out of every channel it doesn't need.
They are independent: `#[server]` keeps a field off the wire but it is
still saved (and so in backups and reports); `#[transient]` keeps it off
disk but a shared cell still sends it.

## The rule for a plaintext secret

**A plaintext secret is never a method's argument and never a cell's
state.** Arguments are in the log for as long as the log is kept; state
is saved and, in a shared cell, sent.

- Take the secret in a plain function of the app (not a method), derive
  what it needs, and keep the result in memory the app owns (a `static`,
  a resource held by the handle).
- Let only a non-secret result reach a method: a flag, a public key, a
  ciphertext.
- Make that method one the network can't call (no `#[client]`), so
  "unlocked" can't be set without the unlock path having run.

```rust
use aio::prelude::*;
use std::sync::Mutex;

/// The key in memory only: never in a cell, never in a call.
static KEY: Mutex<Option<u64>> = Mutex::new(None);

#[cell(persist, shared)]
pub struct Safe {
    pub unlocked: bool,
    /// Sealed with the key: safe to save and send.
    pub sealed: Vec<u8>,
}

#[methods]
impl Safe {
    /// Not #[client]: only the app's own code may say it is unlocked.
    pub fn set_unlocked(&mut self, on: bool) {
        self.unlocked = on;
    }
}

/// The passphrase stays in here.
pub fn unlock(h: &Handle<Safe>, passphrase: &str) -> Result {
    ensure!(passphrase.len() >= 12, "a passphrase has at least 12 characters");
    *KEY.lock().unwrap() = Some(derive(passphrase));
    h.set_unlocked(true) // only a bool reaches the log
}

/// A stand-in: derive a real key with Argon2 or scrypt.
fn derive(passphrase: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    passphrase.hash(&mut h);
    h.finish()
}

# fn main() {
let h = Handle::<Safe>::memory();
unlock(&h, "correct horse battery staple").unwrap();
assert!(h.state().unlocked && KEY.lock().unwrap().is_some());
let log = serde_json::to_string(&h.store(|s| s.events().unwrap())).unwrap();
assert!(!log.contains("correct horse"), "the passphrase is in no event");
# }
```

## `#[redact]`

`#[redact]` on a `pub fn (&mut self, …)` method keeps its arguments out
of the log (aio's `redactActions`); `#[cell(redact)]` does it for every
method of the cell (aio's `redactActions: ["vault:*"]`). For a method
that must take a secret, and keeps only what it derives from it:

```rust
use aio::prelude::*;

#[cell(persist)]
pub struct Lock {
    #[server]
    pub pin_hash: u64,
    pub open: bool,
    pub fails: u32,
}

fn hash(pin: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new(); // a stand-in for a real KDF
    pin.hash(&mut h);
    h.finish()
}

#[methods]
impl Lock {
    #[redact]
    pub fn set_pin(&mut self, pin: String) -> Result {
        ensure!(pin.len() >= 6, "a PIN has at least 6 digits");
        self.pin_hash = hash(&pin);
        Ok(())
    }

    #[redact]
    pub fn try_pin(&mut self, pin: String) {
        match hash(&pin) == self.pin_hash {
            true => self.open = true,
            false => self.fails += 1,
        }
    }
}

# fn main() {
let h = Handle::<Lock>::memory();
h.set_pin("482913".into()).unwrap();
h.try_pin("000000".into()).unwrap();
h.try_pin("482913".into()).unwrap();
assert!(h.state().open && h.state().fails == 1);

let events = h.store(|s| s.events().unwrap());
assert!(events.iter().all(|e| e.args.get(aio::REDACTED).is_some()), "each keeps the state it left");
assert!(!serde_json::to_string(&events).unwrap().contains("482913"), "never the PIN");
# }
```


- Its event keeps the state the call left instead of the arguments
  (`{"$redacted": <state>}`, `aio::REDACTED`), so replay, time
  travel and synced copies still work.
- Wherever events are shown (`ax events`, `ax ui`'s timeline, `ax
  record`) it says `[redacted]`.
- The cell's state never reaches a problem report: the report names the
  cell as withheld.
- Since the state it left is what is kept, that state is in the log:
  `#[redact]` hides the arguments, not what the method stored. A secret
  stored as it came in is in that event; store what is derived from it
  (a hash, a ciphertext), or keep the secret out of the cell (above).
- It can't change a `#[transient]` field (replay couldn't bring it back):
  the call is refused, saying so.
- An async method's steps are redacted too.

## Field flags

| Flag | Saved | Sent to clients | In reports |
|---|---|---|---|
| (none) | yes | yes, in a shared cell | yes |
| `#[server]` | yes | never | yes, unless a `#[redact]` method withholds the cell |
| `#[transient]` | never (back at its default after a restart) | yes, in a shared cell | no |
| `#[server]` and `#[transient]` | never | never | no |

- A shared cell's field named like a credential fails the build unless it
  is `#[server]` (or the cell has a `visible` filter): `password`,
  `passphrase`, `mnemonic`, `private_key`, `api_key`, `secret_key`,
  `access_token`, `auth_token` and their camel case forms. Names that are
  only about one (`api_key_id`, `password_at`, `public_key`) pass.
  Rename the field if it isn't a secret.
- `#[cell(visible = f)]` decides per viewer (only the owner sees their
  own row) ([sync](../state/sync.md#what-each-viewer-sees)).
- `#[cell(save = f)]` shapes what is saved (drop or seal a field on its
  way to disk) ([persistence](../persistence/README.md)).

## What logs and reports never hold

- Every log line and every report is masked: the value after `token=`
  and `TOKEN=` (URLs, the environment), after `Bearer ` and after `pair
  code: ` becomes `…`. That covers aio-rs's own lines; a secret the app
  logs in another shape is written as it is: don't log it.
- The app's key (`AIORT_TOKEN`, `data/sync.token`) and session tokens are
  never logged; `auth.db` keeps only hashes of sessions and passwords.
- A problem report lists the newest calls by name and time, never their
  arguments, and carries the saved state (`#[transient]` fields left out,
  `#[redact]` cells withheld), masked.
- Reports stay on the machine (`data/reports/`, owner-only) unless
  `[feedback] url` or `aio::report::sink` sends them on
  ([debugging](../debugging/README.md#5-problem-reports-and-health)).

## What the tools see

- A debug build's local door has the operator's rights: `ax state` shows
  `#[server]` fields, and time travel and `ax replay` act on the log. A
  release build's door gets what a client with the key gets
  ([the local door](README.md#the-local-door-and-the-window)).
- `ax snapshot`, `ax backup` and the store itself hold the saved state
  raw, `#[server]` fields included: treat a backup as a secret.
- `data/` and `logs/` are owner-only folders (mode `0700` on Unix), and
  `sync.token`, profiles (`ax profile --out`) and reports owner-only
  files: another user of the machine reads none of it; any process of
  yours can.

## Secrets the app needs

- A provider's API key, a webhook's signing secret: from the environment
  or a file in `data/`, never `aio.toml` (it is built into the binary)
  and never the source.
- Read it at start, in the app's own code; hand a method only what it
  must store, `#[redact]` and `#[server]`.
- A service's per-caller tokens: `aio::auth::Users::map` from the
  environment ([accounts](README.md#users-the-app-resolves-itself)).

## Testing code that talks to the outside

A method that opens its own connection can't be tested without the
endpoint. Pass the transport in instead (a trait the test implements),
or record the real exchange once and replay it
([cassettes](../testing/cassettes.md)). Outbound requests made with
`aio::capabilities::agent` are held to `[capabilities] network`, so a
test can't reach a host by accident.

## Checklist

- [ ] No plaintext secret in any cell's state: memory the app owns instead.
- [ ] No plaintext secret as a method argument, or the method is `#[redact]`.
- [ ] Secret fields `#[server]`, and `#[transient]` when they need not survive a restart.
- [ ] Methods that change authority are not `#[client]`, or an `allow` rule guards them.
- [ ] Nothing secret in `aio.toml` or the source; the environment or `data/`.
- [ ] Backups and `ax snapshot` output kept as safely as the key.
- [ ] `ax lint` clean (it flags credentials in shared state).

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::Cell`
(`REDACTED`), `aio::REDACTED`, `aio::Handle::store`, `aio::report` (`redact`, `sink`),
`aio::auth::Users`, `aio::capabilities`.

# Accounts, keys and security

A served app is closed by default: every socket, route and file needs the
app's key, or a signed-in session when accounts are on. Methods are
callable from the network only when they say so (`#[client]`), and what a
viewer is sent is the app's choice (`visible`, `#[server]`)
([sync](../state/sync.md)). This page is everything between the network
and a cell: who may reach the app, how a caller proves who they are, what
they may do, and what aio-rs does not protect. Feature `server` at least;
`tls` for TLS and pairing, `oidc` for identity providers (`net` has all
three).

| You want | Use |
|---|---|
| a tool for yourself on this machine | nothing: loopback and the key, as made |
| the app on your phone or another PC on the LAN | `--expose` (TLS on), `ax pair` |
| a small team with logins | `[accounts] on = true`, `ax auth <app> create` |
| sign-in with an identity provider | `[accounts]` and `Http::oidc` (feature `oidc`) |
| a service whose callers are machines | `Http::users(Users::map(..))`: a token per caller |
| a public site | a reverse proxy in front: `[server] hosts`, `trust_proxy`, your own certificate |

## Who can reach the app

A served app (`[app] mode = "client-server"` or `"server"`) listens where
`[server] listen` says, and a launch can move it:

| Launch | Listens on | TLS |
|---|---|---|
| nothing | `[server] listen`, else `127.0.0.1:0` (a free port) | as `[server] tls` |
| `--port 8100` | that port, on `listen`'s host | as `[server] tls` |
| `--host 192.168.1.20` | that interface only (a LAN card, a VPN address) | on, unless it is loopback |
| `--expose` | `0.0.0.0` (every interface) | on |

- A launch that opens the app to the network (`--expose`, a host not on
  loopback) turns TLS on, so the key never crosses a LAN in the clear.
  `--expose` and `--host` both say where to listen: give one.
- `[server] listen = "0.0.0.0:8100"` with `tls = false` serves plain HTTP
  to the network; the boot says so as a `security:` warning naming the
  fix (`tls = true`, or a TLS proxy in front).
- The boot line says which of these decided and how open the app is:
  `team listens on 127.0.0.1:8100 (this machine only; [server] listen)`,
  `team is open to the network on 0.0.0.0:8100 over TLS, behind its key
  (--expose)`.
- A port in use stops the boot at once, naming the address.
- A service gets these flags in its supervisor file's command line; `ax
  start <app> -- --expose` passes them to a background launch.

Even on loopback the key is asked for: a page in a browser on the same
machine can reach `127.0.0.1`, and so can every other user's process.

## The key

A served app has one key: `AIORT_TOKEN`, else `data/sync.token` (made
once, 64 random hex characters, owner-only). It is never in `aio.toml` (which is
built into the app) and never logged; a weak one set by hand (under 16
characters or 80 bits) is said at boot.

- **Showing it**: `Authorization: Bearer <key>`, `?token=<key>` or the
  cookie a `?token=` link sets. A key in a URL is said once in the log:
  URLs leak into history, referrers and proxy logs.
- **The browser cookie**: opening `https://host:port/?token=<key>` sets
  `aio_key_<app id>` (HttpOnly, `SameSite=Strict`, `Secure` over TLS), so
  the page's own requests (its scripts, styles, `/ws`) carry it without
  the key in every URL. It is named per app: cookies ignore the port.
- **Guessing**: an address that shows 10 wrong keys or sessions within 5
  minutes is refused with 429 and `Retry-After`, 5 s at first, then 10,
  20, 40 and 60 s while it keeps guessing (a strike is forgotten after 10
  minutes). A missing key is not a guess, and a caller with a valid key
  is always served, so nobody can lock others out from a shared address.
  An IPv6 address counts by its /64.
- **Rotating it**: stop the app, delete `data/sync.token` (or set a new
  `AIORT_TOKEN`), start it; every client pairs again.
- **Handing it to a client**: `ax profile <app> [--out file]` writes a
  client profile (`.aioapp`: host, port, root, key), `--out` writes it
  owner-only. A paired device gets it with an 8-digit code instead
  ([pairing](#pairing-a-device)).
- **No key at all**: `[server] open = true` serves with none: anyone who
  reaches the port reads the state and calls `#[client]` methods. It is
  said loudly at boot and by `ax doctor`, minting a pairing code still
  needs the key, and it is refused together with `[accounts]`.

```toml
[app]
id = "notes"
mode = "client-server"

[server]
listen = "127.0.0.1:8100"
```

```sh
AIORT_TOKEN="$(openssl rand -base64 24)" ./notes --expose  # a key of your own
ax profile notes --out notes.aioapp                         # the key, for another client
```

## Pairing a device

`[server] pairing = true` (feature `tls`) lets a new device get the key
with a short code: `ax pair <app>` asks the running app for one, and the
device runs `ax pair <app> <host:port> <code>`.

- The code is 8 digits and never crosses the wire: both sides run CPace
  (a PAKE) bound to the TLS certificate the device sees, so a relay with
  a certificate of its own learns nothing and the code is not spent.
  `ax pair <app> --plain` makes a 6-digit code that is sent as it is, for
  aio's older clients.
- A code lives 3 minutes and works once. 8 wrong guesses from one address
  lock that address out; 20 wrong in all burn the code (`ax pair <app>` makes a
  new one, no restart).
- The device keeps the profile and pins the app's root certificate, never
  the OS trust store.

Details, LAN discovery and peers: [sync](../state/sync.md#pairing-and-discovery).

## Accounts

`[accounts] on = true` gives the app users with roles, password logins,
TOTP second factors and sessions, in `data/auth.db` (aio's tables and
hash format: the file moves between aio and aio-rs both ways). The shared
key then no longer opens sync: a session does, and the user is the
session's, never what a client says.

```toml
[app]
id = "team"
mode = "server"

[server]
listen = "0.0.0.0:8100"
tls = true

[accounts]
on = true
signup = false            # POST /__aio/auth/signup for anyone (default: off)
session = "30d"           # unused this long, a session ends (used past half, renewed)
max_sessions = 10         # per user; a new one past it ends the oldest
require_verified = false  # no sign-in until the email is verified
```

| `[accounts]` key | Default | Means |
|---|---|---|
| `on` | `false` | accounts serve the app; the shared key no longer opens sync |
| `signup` | `false` | anyone may create an account; off: only the app or `ax auth` makes them |
| `session` | `"30d"` | how long a session lives unused, `1s` to 100 years (`s`, `m`, `h`, `d`) |
| `max_sessions` | `10` | live sessions per user; a new one past it ends the oldest (also at boot, for a lowered cap) |
| `require_verified` | `false` | no sign-in until the email is verified; sign-up then needs an email |

`aio::auth::Auth` is the same thing as a library: what the server runs,
and what a test or a tool opens on an `auth.db`:

```rust
use aio::auth::{Auth, Login, Policy};

# fn main() {
let auth = Auth::memory(Policy::default()).unwrap();
auth.create("ann", "correct horse battery", Some("admin")).unwrap();
auth.create("bob", "correct horse battery", None).unwrap();

let Login::Signed { token, user } = auth.login("ann", "correct horse battery").unwrap() else {
    panic!("no second factor set")
};
assert_eq!(user.role, "admin");
let session = auth.session(&token).unwrap().unwrap();
assert_eq!(session.user, "ann");

assert!(auth.login("bob", "wrong").is_err());
auth.set_role("bob", "editor").unwrap(); // counts from bob's next call
auth.revoke(&token).unwrap();
assert!(auth.session(&token).unwrap().is_none());

// A second factor: enrol with one code, then each login asks for the next.
let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
let secret = auth.totp_setup("bob").unwrap(); // shown once, as a QR code or text
auth.totp_enable("bob", &aio::testing::totp_code(&secret, now - 30_000)).unwrap();
let Login::Totp { pending } = auth.login("bob", "correct horse battery").unwrap() else {
    panic!("bob has a second factor now")
};
let (_token, bob) = auth.login_totp(&pending, &aio::testing::totp_code(&secret, now)).unwrap();
assert_eq!(bob.role, "editor");
# }
```

### Users and roles

- A user is an id, a role (`"user"` unless given), an optional email and
  whether it is verified, and whether TOTP is on (`aio::auth::User`: never
  the password hash or the TOTP secret).
- Ids are one key however they are typed: Unicode NFC and trimmed; empty,
  over 256 characters, whitespace, control and invisible (format)
  characters refused (`invalid_id`); unique by their case fold, so
  `Ａdmin` (full width) can't stand beside `admin` (`user_exists`).
- `oidc:` starts the ids of identity providers' accounts: no sign-up may
  claim it (`reserved_id`).
- A role is any string the app gives meaning to. It is read from the
  account on every call, never kept in the client: `set_role` counts from
  the user's next call, and a removed user's sockets close.
- `Auth` methods: `create`, `create_external`, `user`, `users`,
  `set_role`, `set_email`, `mark_verified`, `set_password`, `unlock`,
  `remove`, `end_all`, `login`, `login_totp`, `totp_setup`,
  `totp_enable`, `totp_off`, `issue`, `session`, `refresh`, `revoke`,
  `revoke_user`, `sessions`. A refusal is an `aio::auth::Refusal` whose
  `code()` is aio's wire code.

### Passwords and lockout

- Passwords: at least 8 characters (`password_too_short`), hashed with
  PBKDF2-HMAC-SHA-256, 600 000 iterations and a 16-byte salt, as
  `pbkdf2$sha256$<iterations>$<salt>$<hash>` (aio's format; the count is
  in the hash, so it can be raised later). An unknown id costs the same
  hash, so timing tells nothing.
- **The account**: 5 wrong passwords or codes in a row lock it for 15
  minutes. While locked, only the right password is told so (423
  `account_locked`); a wrong one still gets 401, so the lock is no
  oracle. A right password resets the count; with TOTP on, only a right
  code does. Checks for one account run one at a time, so a burst gets
  exactly the tries one-by-one attempts get. `ax auth <app> unlock <id>`
  lifts a lock.
- **The address** (per IPv4 address or IPv6 /64), on top:

| Budget | Allows | Then |
|---|---|---|
| password checks started | 30 a minute (a right one gives its unit back) | 429 `too_many_attempts` |
| failed sign-ins | 10 per 5 minutes | 429 `too_many_attempts` |
| new accounts | 10 an hour | 429 `too_many_accounts` |
| mails per address and per account | 10 per 5 minutes | not sent: the anonymous doors answer the same, `verify/request` 429 |

A right password is always served, whatever the address spent, so a
crowd behind one NAT can't lock a user out; the per-account lock is what
rotating addresses can't sidestep. Behind a proxy, set `[server]
trust_proxy`, or every client shares the proxy's budgets
([proxy](#behind-a-reverse-proxy)).

### Sessions

- A session token is `aios_` and 256 random bits, shown once; `auth.db`
  keeps only its SHA-256, so a stolen file holds no usable token.
- It lives `[accounts] session` unused; used past half its life it is
  renewed (sliding). `max_sessions` ends the oldest past the cap, said in
  the log.
- It ends at logout, `revoke`, `revoke_user` (a kick, a breach), a new
  password (every session of the user, and every pending mail token), a
  removed user, or expiry. Every end closes that session's open sockets
  at once.
- A client shows it as `Authorization: Bearer`; a browser keeps it in the
  cookie a sign-in sets: `aio_session_<app id>`, HttpOnly,
  `SameSite=Strict`, `Secure` over TLS or behind a trusted proxy that
  says https. A session never goes in a URL: `/ws?token=<session>` is
  refused. A page that sends `x-aio-session: cookie` gets no token in the
  answer at all, so its script never holds one.
- Every auth answer is `Cache-Control: no-store`.
- With accounts, the app's page itself (the shell) is served without a
  key, so the sign-in form can load; everything behind it needs a
  session.

### TOTP

A second factor as every authenticator app speaks it (RFC 6238):
HMAC-SHA-1, 30-second steps, 6 digits, a 160-bit base32 secret.

1. `POST /__aio/auth/totp/setup` (signed in) stages a secret and answers
   it with its `otpauth://` URI, shown once as a QR code or text.
2. `POST /__aio/auth/totp/enable` with `{ password, code }` turns it on:
   the password, not only the session, and a code that proves the
   person's app has the secret.
3. A login then answers `{ totpRequired: true, pending }`; `POST
   /__aio/auth/totp` with `{ pending, code }` within 5 minutes gives the
   session. Any answer spends `pending`, so a wrong code goes back to the
   password.

- A code works once, even across a restart (the step used is kept with
  the account), within one step of clock skew each way.
- Wrong codes count toward the same lockout as wrong passwords.
- `totp/disable` with `{ password }` turns it off; for a lost phone the
  operator runs `ax auth <app> totp <id> off`.

### Email verification and password reset

Both need the app's mail hook: `Http::mail(|m| ..)` gets an
`aio::auth::Mail` (`to`, `subject`, `text`) and sends it however the app
sends mail (SMTP, a provider's API). Without it those doors answer 501
`mail_not_configured`, and `ax auth <app> verify <id>` verifies by hand.

```rust
use aio::auth::Mail;
use aio::http::Http;

fn http() -> Http {
    Http::default().mail(|m: &Mail| {
        // Hand it to your mail service; an Err is logged, never shown to the asker.
        aio::info!(cat: "mail", to = m.to.as_str(), "{}", m.subject);
        Ok(())
    })
}

# fn main() { let _ = http(); }
```

- **Verification**: sign-up with an email sends a token (valid 24 hours);
  `POST verify` with `{ token }` marks the email verified. `verify/request`
  (signed in) sends a new one; `verify/resend` with `{ id }` (anonymous)
  is the way back for an account whose token expired.
- **Reset**: `reset/request` with `{ id }` mails a token valid 15
  minutes; `reset` with `{ token, password }` sets the password, ends
  every session and marks the email verified (the mailbox is proven).
  A too-short password is refused before the token is spent.
- The anonymous doors (`verify/resend`, `reset/request`) answer 200
  whether or not the account exists, do the same work either way and
  send the mail off the answer's path, so they tell a prober nothing.
  Each is budgeted per address and per account, so an inbox is no
  mail-bomb target.
- An identity provider's account has no password here: a reset for it is
  refused (`external_identity`).
- With `require_verified`, a sign-up gets no session until the email is
  verified (`201 { verificationSent }`), and a login of an unverified
  account is 403 `email_unverified`.

### The login doors

The server serves them at `/__aio/auth/*` when accounts are on. Bodies
are JSON (16 KB at most); answers are JSON with `error` set to a code on
a refusal.

| Door | Body | Answers |
|---|---|---|
| `GET me` | | `{ user: {id, role} \| null, features: {signup, totp, oidc, mail} }` |
| `POST signup` | `{ id, password, email? }` | 201 `{ user, token }`, or `{ user, verificationSent }` |
| `POST login` | `{ id, password }` | `{ user, token }` or `{ totpRequired, pending }` |
| `POST totp` | `{ pending, code }` | `{ user, token }` |
| `POST logout` | | `{ ok }`, the cookie cleared |
| `POST password` | `{ old, new }` (signed in) | `{ user, token }`: a new session, all others ended |
| `POST totp/setup` | (signed in) | `{ secret, uri }` |
| `POST totp/enable` | `{ password, code }` (signed in) | `{ ok }` |
| `POST totp/disable` | `{ password }` (signed in) | `{ ok }` |
| `POST verify/request` | (signed in) | `{ ok }` |
| `POST verify/resend` | `{ id }` | `{ ok }`, always |
| `POST verify` | `{ token }` | `{ ok }` |
| `POST reset/request` | `{ id }` | `{ ok }`, always |
| `POST reset` | `{ token, password }` | `{ ok }` |
| `GET oidc/start?redirect=/path` | | 302 to the provider ([OIDC](#sign-in-with-an-identity-provider)) |
| `GET oidc/callback` | | 302 back to the page, signed in |

| Code | Status | Means |
|---|---|---|
| `invalid_credentials` | 401 | wrong id or password (never which) |
| `account_locked` | 423 | the right password, but the account is locked for now |
| `too_many_attempts` | 429 | this address spent its budget; wait |
| `too_many_accounts` | 429 | this address made 10 accounts this hour |
| `invalid_code`, `pending_expired` | 401 | a wrong TOTP code; the second step came too late |
| `login_required` | 401 | the door needs a session |
| `signup_disabled` | 403 | `[accounts] signup` is off |
| `email_unverified` | 403 | `require_verified` and the email isn't yet |
| `invalid_id`, `reserved_id`, `password_too_short` | 400 | the id or password can't be used |
| `invalid_email`, `email_required` | 400 | the email doesn't look like one; verification needs one |
| `user_exists`, `totp_already_enabled` | 409 | the id is taken; TOTP is on already |
| `setup_first` | 400 | `totp/enable` before `totp/setup` |
| `invalid_or_expired_token` | 401 | the mailed token is wrong, spent or expired |
| `external_identity` | 403 | an identity provider's account has no password here |
| `mail_not_configured`, `mail_failed` | 501, 503 | no mail hook; the hook failed |
| `auth_unavailable` | 503 | `auth.db` couldn't be read or written (logged) |

`aio::auth::client::text(code, status)` is the sentence a person can act
on for each ("Incorrect username or password.").

### Signing in from a client

`aio::auth::client::Client` speaks the doors: a native app, a tool or a
test signs up or in, then links with the session as its key. In a page
(the web build) it asks the server that served the page, and the session
stays in the HttpOnly cookie.

```rust
use aio::auth::client::{Client, Login, Signup};
use aio::auth::{Auth, Policy};
use aio::prelude::*;
use aio::sync::{Options, Remote};
use aio::testing::TestServer;
use std::sync::Arc;

#[cell(persist, shared)]
pub struct Team {
    pub notes: Vec<(String, String)>,
    pub banner: String,
}

#[methods]
impl Team {
    #[client]
    pub fn note(&mut self, cx: &Cx, text: String) -> Result {
        // The proven caller: the session's user, never what the client says.
        let By::User(who) = cx.by() else { reject!("sign in first") };
        self.notes.push((who.clone(), text));
        Ok(())
    }

    #[client(role = "admin")]
    pub fn set_banner(&mut self, text: String) {
        self.banner = text;
    }
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
rt.block_on(async {
    let auth = Arc::new(Auth::memory(Policy { signup: true, ..Default::default() }).unwrap());
    auth.create("ann", "correct horse battery", Some("admin")).unwrap();
    let h = Handle::<Team>::memory();
    let opts = Options { auth: Some(auth.clone()), ..Default::default() };
    let server = TestServer::start_with(h.clone(), opts).await.unwrap();

    // With accounts on, the shared key opens nothing.
    assert!(Remote::<Team>::connect(&server.addr, &server.token, "ann").await.is_err());

    let c = Client::new(&server.addr, None);
    let Signup::Signed(bob) = c.signup("bob", "a long passphrase", None).await.unwrap() else {
        panic!("no verification asked")
    };
    assert_eq!(bob.role, "user");
    let Login::Signed(ann) = c.login("ann", "correct horse battery").await.unwrap() else {
        panic!("no second factor")
    };
    let e = c.login("ann", "wrong").await.unwrap_err();
    assert_eq!((e.code.as_str(), e.status), ("invalid_credentials", 401));

    // A copy of the cell, signed in as each.
    let bob_copy = Remote::<Team>::connect_to(c.link(&bob), "bob").await.unwrap();
    let ann_copy = Remote::<Team>::connect_to(c.link(&ann), "ann").await.unwrap();
    let refused = tokio::task::spawn_blocking(move || {
        bob_copy.call_confirmed("note", serde_json::json!(["hello"])).unwrap();
        bob_copy.call_confirmed("set_banner", serde_json::json!(["mine now"]))
    });
    assert!(refused.await.unwrap().is_err(), "bob is no admin");
    tokio::task::spawn_blocking(move || ann_copy.call_confirmed("set_banner", serde_json::json!(["hi all"])))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(h.state().notes, vec![("bob".to_string(), "hello".to_string())]);
    assert_eq!(h.state().banner, "hi all");

    c.logout(&bob.token).await.unwrap();
    assert_eq!(c.me(Some(&bob.token)).await.unwrap().user, None);
});
# }
```

- `Client::new(addr, root)`: `root` is the PEM to pin over TLS (a
  paired profile has it), `None` for plain HTTP.
- Methods: `me`, `signup`, `login`, `totp`, `logout`, `password`,
  `totp_setup`, `totp_enable`, `totp_disable`, `verify_request`,
  `verify_resend`, `verify`, `request_reset`, `reset`; `link(&signed)`
  is the `aio::sync::Link` for `Remote::connect_to`.
- A refusal is an `AuthError`: `code` to branch on, `status`, and a
  `Display` that is the sentence for a person.

### The sign-in form

`aio::kit::SignIn` (feature `kit`) is a ready form over `Client`: sign
in, create an account when the server allows it, the second-factor step,
and "Forgot password?" when the server sends mail. It asks `GET me` and
adapts: no sign-up toggle with sign-up off, no reset without mail. Props:
`client`, `on_signed` (gets the `Signed` session), `title`, `signup`,
`email`, `forgot` (each on by default), `class`. Its parts
(`signinId`, `signinPassword`, `signinCode`, `signinSubmit`,
`signinError`…) are named for UI tests.

### Users the app resolves itself

Without accounts, an app may know its callers by their token: a
service's machines, a fixed set of devices, an API key per user.

```rust
use aio::auth::Users;
use aio::http::Http;

fn http() -> Http {
    let tokens = [
        (std::env::var("BUILD_BOT_TOKEN").unwrap_or_default(), ("build-bot".to_string(), "ci".to_string())),
        (std::env::var("ANN_TOKEN").unwrap_or_default(), ("ann".to_string(), "admin".to_string())),
    ];
    Http::default().users(Users::map(tokens))
}

# fn main() {
let users = Users::map([("k".repeat(32), ("ann".to_string(), "admin".to_string()))]);
assert_eq!(users.who(&"k".repeat(32)).unwrap().role, "admin");
assert!(users.who("guess").is_none());
let _ = http();
# }
```

- A token that resolves is that user with that role on every door
  (sockets, routes, files, server functions); the shared key then opens
  none of them, so every caller is someone.
- `Users::map` compares every entry in constant time and says a weak
  token at boot; `Users::resolve(|token| ..)` is the app's own lookup
  (one that panics resolves no one). An empty token resolves no one.
- Keep the tokens out of the source: the environment or a file in
  `data/`.

### Operator console: `ax auth`

`ax auth <app> <do> [id] [value]` works on the app's `auth.db` straight,
running or not: it seeds the first admin before the app ever ran, and it
is the way back in when locked out. A change reaches the running app's
next request (sessions and roles are read each time).

| Command | Does |
|---|---|
| `ax auth team users` | every user: role, email, verified, 2FA, locked, created |
| `ax auth team create ann --role admin --email ann@example.org` | a new user; no `--password`: one is made and shown once |
| `ax auth team create oidc:login.example.org:1234 --role editor` | an identity provider's account, ahead of its first sign-in |
| `ax auth team passwd ann` | a new password (shown once; `--password p` sets it), lockout cleared, sessions ended |
| `ax auth team role ann editor` | a new role, from the next call |
| `ax auth team totp ann off` | the second factor cleared (a lost phone) |
| `ax auth team unlock ann` | the lockout lifted |
| `ax auth team verify ann` | the email marked verified |
| `ax auth team revoke ann` | every session of the user ended |
| `ax auth team rm ann` | the user removed, sessions ended |

Every command takes `--json`. A password on the command line lands in
the shell's history: leave `--password` out and copy the one shown.

`aio::auth::serving()` is the running server's `Auth`, for a route or
server function that manages accounts (an admin screen), or that signs a
user in by a flow of the app's own (`auth.issue(user, role)` mints a
session).

## Who may call what

Getting in is half of it; each call is decided again, on the server:

| Where | Rule | Decides on |
|---|---|---|
| a method | no `#[client]` | the network never calls it (the app's own code, effects and schedules do) |
| | `#[client]` | anyone who got in (the key, a session, a resolved token) |
| | `#[client(role = "admin")]` | a session or token with that role now |
| | `#[client(allow = f)]` | `f(&self, &Caller, args…) -> bool` on the state the call would run on (row-level) |
| in a method | `cx.by()`, `cx.role()` | the proven caller and their role, saved with the event so replay sees the same |
| what a viewer sees | `#[cell(visible = f)]`, `#[server]` fields | per viewer, before it is sent |
| a server function | `Access::Open`, `Signed`, `Role("x")`, `Check(rule)` | the caller (`Req`) and the arguments; fails closed |
| a route | the key or a session, unless `Route::public()` | `req.user()`, `req.role()`, `req.ip()` |

- Without accounts or resolved users, the shared key proves no one: only
  `Access::Open` lets a call in, and `cx.by()` is the name the client
  gave, a claim.
- `ax peer <app> [--as user] [--role r]` lists what a caller may call,
  with each method's signature and rule; `TestCells::as_client(&h,
  caller)` tests the rules as a network caller meets them.
- A public route (a webhook) checks its own signature: nothing else
  stands in front of it.

Examples and the rest: [sync: who may call what](../state/sync.md#who-may-call-what),
[what each viewer sees](../state/sync.md#what-each-viewer-sees),
[server functions](../state/sync.md#server-functions-and-routes).

## Sign-in with an identity provider

Feature `oidc`, with accounts on: `Http::oidc(Oidc::new(issuer,
client_id))` adds the OpenID Connect authorization code flow with PKCE
(S256). A page links to `/__aio/auth/oidc/start?redirect=/path`; the
provider sends the browser back to `/__aio/auth/oidc/callback`, which
signs it in (the session cookie) and redirects to `/path`.

```rust
# #[cfg(feature = "oidc")]
fn http() -> aio::http::Http {
    use aio::auth::oidc::Oidc;
    let provider = Oidc::new("https://login.example.org", "team-app")
        .secret(std::env::var("OIDC_SECRET").unwrap_or_default()) // leave out for a public (PKCE-only) client
        .signup(true) // a new identity gets an account at its first sign-in
        .role(|claims| match claims["email"].as_str() {
            Some(e) if e.ends_with("@example.org") => Ok("member".into()),
            _ => Err("this app is for example.org accounts".into()),
        });
    aio::http::Http::default().oidc(provider)
}

# fn main() {
# #[cfg(feature = "oidc")]
# let _ = http();
# }
```

```toml
[capabilities]
network = ["login.example.org"]   # the provider: outbound requests are held to this list
```

| `Oidc` | Default | Means |
|---|---|---|
| `new(issuer, client_id)` | | the provider, found from `<issuer>/.well-known/openid-configuration` |
| `.secret(s)` | none | a confidential client's secret; none: a public client (PKCE only) |
| `.signup(open)` | `[accounts] signup` | may an identity with no account get one at its first sign-in |
| `.role(f)` | `"user"` | a new account's role from its verified claims; an `Err` refuses the sign-in |
| `.redirect_uri(uri)` | `<scheme>://<host>/__aio/auth/oidc/callback` | the callback as registered with the provider (behind a proxy) |

- Discovery's `issuer` must be the one configured; it and the provider's
  keys are kept an hour, and a token naming a key not yet seen reads them
  again (a rotated key), at most every 30 s.
- ID tokens are checked: RS256 or ES256 against the provider's keys,
  `iss`, `aud`, `azp`, `exp`, `nbf`/`iat` (60 s skew), `sub` and the
  login's `nonce`.
- The state is one-shot (10 minutes), carries the PKCE verifier, the
  nonce and the return path, and is bound to a cookie set at the start,
  so a callback replayed in another browser is refused. The return path
  is a path on this app only, never another site.
- Accounts are `oidc:<issuer host and path>:<sub>`, a namespace of their
  own: no local account is adopted by a matching email, and they have no
  password. An email counts only with `email_verified: true`. Existing
  accounts keep their role; `ax auth <app> role` changes it.
- `GET me` says `oidc: true`, so a sign-in page knows to show the button.

## TLS and the network

- `[server] tls = true` (or a launch that exposes the app) serves HTTPS
  and `wss://` with a certificate from this machine's own root: one root
  per user (ECDSA P-256, 10 years), and a fresh leaf for each app at each
  start, naming this machine's addresses.
- The root is name-constrained to `localhost`, `.local`, loopback and
  private and link-local addresses, so even a stolen root key can't vouch
  for a public site. Clients pin it (pairing and profiles hand it out),
  never the OS trust store.
- A browser warns about it until told to trust it: `ax trust` names the
  root file and the command that adds it to this OS's store (it installs
  nothing itself); `ax trust path` prints the file alone, to copy to
  another device. Firefox keeps a store of its own.
- The operator's own certificate: `[server] cert` and `key` (PEM files,
  absolute paths, `tls = true`), e.g. one a public CA signed.
  `[server.public_dirs]` `"/.well-known" = "acme"` serves an ACME
  client's challenge files without the key. Only then is HSTS sent.

```toml
[server]
listen = "0.0.0.0:8443"
tls = true
cert = "/etc/team/fullchain.pem"
key = "/etc/team/privkey.pem"
hosts = ["team.example.org"]

[server.public_dirs]
"/.well-known" = "acme"
```

## Hosts, origins and forged requests

Every request passes one gate before any route, file or socket:

- **Host**: it must be addressed to this app: an IP literal, `localhost`,
  a `.local` name, or a name in `[server] hosts`. Any other name is 403:
  that is the shape of DNS rebinding (a page whose DNS flips to this
  machine would otherwise be same-origin with the app).
- **Origin**: absent (native clients, `curl`, tools) is fine; present, it
  must be this same host or one in `hosts`, else 403 naming the origin.
  `strict_origin = true` makes a socket upgrade to an app open to the
  network say its Origin (an origin-stripping proxy would skip the
  check).
- **POST bodies are JSON**: a cross-site form can't send
  `application/json` without a CORS preflight, which nothing answers, so
  a forged form post never runs a route. One exception: a browser
  navigating by a form POST to a client route (a payment's return page,
  a `form_post` response) gets the app's page, and nothing runs.
- Cookies are `SameSite=Strict`; a header credential (`Authorization`)
  is never attached by a browser on its own.
- `[server] hosts` is also the list of sites that may frame the app
  (`frame-ancestors`), so a page that may embed it is exactly one that
  could use it.

## Behind a reverse proxy

Terminate TLS at the proxy (with a public CA's certificate), and tell the
app what the proxy does:

```toml
[app]
id = "team"
mode = "server"

[server]
listen = "127.0.0.1:8100"          # only the proxy reaches it
hosts = ["team.example.org"]       # the name people use
strict_origin = true
trust_proxy = "x-forwarded-for"    # the header the proxy overwrites
```

```nginx
location / {
    proxy_pass http://127.0.0.1:8100;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;     # the socket
    proxy_set_header Connection "upgrade";
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $remote_addr;
    proxy_set_header X-Forwarded-Proto $scheme;
}
```

- With `trust_proxy`, the client is that header's **last** hop (the one
  the proxy wrote; the first is whatever the client sent), so the
  per-address budgets count people, not the proxy. The proxy's
  `X-Forwarded-Proto: https` makes cookies `Secure`.
- Set it only when a proxy overwrites that header: the app warns when
  requests carry a forwarding header with no `trust_proxy` (every client
  shares one address's budgets), and when `trust_proxy` is set but a
  request has no hop the proxy wrote (callers would pick their own
  address).
- A relayed request is never a same-machine one.
- Pairing by PAKE binds the exchange to the certificate the device sees:
  a TLS-terminating proxy is a relay to it. Pair on the app's own address,
  or hand a profile with `ax profile`.
- OIDC behind a proxy: `.redirect_uri(..)` the public callback URL.

## Security headers

Every answer carries headers decided once at boot from `[server.security]`
and what the app already said; a default is on only when it can't break
a working app.

| Key | Default | Means |
|---|---|---|
| `headers` | `true` | `false`: only `X-Content-Type-Options: nosniff` |
| `csp` | `"basic"` | `basic` (`base-uri`, `object-src`, `form-action`, a `script-src` without `'unsafe-eval'`; no `default-src`, so off-origin assets load), `strict` (adds `default-src 'self'` and per-type sources), `off`, or a policy used verbatim |
| `directives` | none | per directive: a value replaces or adds it (`"img-src" = "'self' https:"`), `false` drops it; `{nonce}` names the nonce |
| `nonce` | `false` | with `strict`: the page's scripts carry a fresh nonce per answer instead of `'unsafe-inline'` |
| `frame_options` | `true` | `X-Frame-Options: SAMEORIGIN` when `hosts` names no site that may frame it |
| `referrer` | `"no-referrer"` | `Referrer-Policy` |
| `hsts` | `true` | behind the operator's own certificate only: `true` is 180 days, a string is used verbatim |
| `permissions` | none | `Permissions-Policy`, verbatim (camera, microphone: the app says) |

```toml
[server.security]
csp = "strict"
nonce = true
permissions = "camera=(), microphone=()"

[server.security.directives]
"img-src" = "'self' https: data:"
```

A value no header can carry refuses the boot, naming the key; it is
never a 500 on every answer. A route that sets a header of its own keeps
it.

## Limits per link

`[server.limits]` bounds what one client can cost: `clients` (256 open
links), `max_message` (4 MiB a message), `calls_per_sec` (200, bursts to
twice that) and `bytes_per_sec` (8 MiB, bursts likewise)
([sync](../state/sync.md#server-functions-and-routes)).

## What the app may reach

`[capabilities] network` lists the hosts aio-rs's own outbound requests
may reach (sync, pairing, sign-in, OIDC, peers, updates, feedback, and
`aio::capabilities::agent` for the app's own HTTPS). This machine always;
any other host refused before a byte is sent, saying the line that
allows it.

| Entry | Lets through |
|---|---|
| `"api.example.org"` | that host, any port |
| `"10.0.0.5:8443"` | that host and port |
| `"*.example.org"` | its subdomains |
| `"lan"` | this network: private and link-local addresses, `.local` names |
| `"*"` | anywhere |

The hosts `aio.toml` itself names (`[client] server`, `[updates] url`,
`[feedback] url`) must be in it: the build says which is missing.

## The local door and the window

- The app's own tools (`ax state`, `ax call`, time travel) reach a
  running app through its local door: a private socket (a Unix socket,
  or a named pipe whose DACL names only this user). Another user's
  connection is refused and said.
- Only a debug build's door has the operator's rights (any method,
  `#[server]` fields, time travel). A release build's door gets what a
  client with the key gets: `#[client]` methods as their rules say, and
  what `visible` shows. So does another app calling it as a peer.
- The window's own socket serves only the window process the app
  spawned, told by pid and start time: another program of the same user,
  or a later process handed the pid, is refused.

## Secrets in state

A value leaves the app on the wire (to clients) or into a record (the
event log, logs, reports, backups). `#[server]` keeps a field off the
wire, `#[transient]` off disk, `#[redact]` keeps a method's arguments out
of the log, and a shared cell's field named like a credential fails the
build unless it is `#[server]`. The patterns, and what each channel
holds: [secrets and observability](secrets.md).

## Security model

What aio-rs protects:

- The network can't read or change state without the key, a session or
  a resolved token; TLS whenever the app is open to the network.
- A caller can't claim to be someone else: the user and role are the
  session's, read live.
- Methods, fields and server functions are closed until the app opens
  them (`#[client]`, `visible`, `Access`); a field named like a
  credential in a shared cell fails the build unless `#[server]`.
- Guessing (keys, passwords, codes, pairing codes) is budgeted per
  address and per account; nothing tells a prober whether an account
  exists.
- DNS rebinding, cross-site forms and foreign framing are refused.
- Secrets stay out of logs and reports ([secrets](secrets.md)).

What it does not:

- A same-user process on this machine: it can read `data/sync.token`,
  `auth.db` and the store, and talk to the release door with a client's
  rights. The OS account is the boundary.
- A compromised machine, a stolen backup (`auth.db` holds hashes, not
  passwords; `data/sync.token` is the key itself), or what a user with a
  valid session is allowed to do.
- Denial of service beyond the per-link limits and budgets: put a proxy
  in front of a public app.
- The app's own routes marked `public()`, its server functions with
  `Access::Open`, and its `allow` rules: they are as safe as their code.

## Limits

What aio has that aio-rs does not, or does differently:

- **No sessions without accounts** (aio's `sessions: true`): sessions
  come with `[accounts]`. An app with its own login flow mints them with
  `aio::auth::serving()` and `issue(user, role)`.
- **No TLS flags at launch** (aio's `--tls-cert`, `--tls-key`,
  `--no-tls`): the certificate is `[server] cert` and `key`, and a launch
  that exposes the app always serves TLS; plain HTTP to the network takes
  `[server] listen` with `tls = false`.
- **No CORS answers**: no `Access-Control-Allow-*` headers are sent, so a
  page on another origin can't read the app's routes with `fetch`; it may
  open the socket when its host is in `[server] hosts`. aio's
  `allowedOrigins` entries with a scheme and port are, in aio-rs, host
  names (`[server] hosts`).
- **No share link printed** at boot: `ax profile` or pairing hands the
  key out; a browser link is `https://host:port/?token=<key>` by hand.
- **No `useUser()` hook**: `Client::me` answers who is signed in, and
  `SignIn`'s `on_signed` hands the session to the app.
- **Lockout and budgets are fixed** (5 tries, 15 minutes; the per-address
  table above), as in aio; there are no keys to tune them.
- **No passkeys (WebAuthn)**, as in aio.

## Common mistakes

- Serving beyond loopback without TLS: the key crosses the network in the
  clear (a phone refuses to). Set `[server] tls = true`, or launch with
  `--expose`.
- Deciding access in the view: a client can call the method anyway. Use
  `#[client(role = …, allow = …)]` and `visible`.
- `Access::Open` on a server function that reads private data: the shared
  key proves no one. Use `Signed`, `Role` or `Check` with accounts on.
- Trusting `cx.by()` without accounts: it is the name the client gave.
- `trust_proxy` set with no proxy in front: each caller picks the address
  its budgets count against.
- A provider or a server host missing from `[capabilities] network`: the
  request is refused before it is sent; the error says which line to add.
- Token tables in the source (`Users::map` with literals): read them
  from the environment or `data/`.
- A password in `ax auth … --password` on a shared machine: it is in the
  shell's history.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::auth`
(`Auth`, `Policy`, `Login`, `User`, `Session`, `Users`, `Mail`,
`Refusal`, `serving`), `aio::auth::client` (`Client`, `AuthError`),
`aio::auth::oidc` (`Oidc`), `aio::auth::budget`, `aio::kit::SignIn`,
`aio::fns::Access`, `aio::http::Http` (`users`, `mail`, `oidc`),
`aio::security`, `aio::capabilities`, `aio::tls`, `aio::pairing`,
`aio::gate`.

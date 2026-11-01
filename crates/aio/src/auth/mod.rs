//! Accounts: users with roles, password logins, TOTP second factors and
//! sessions, in the app's `auth.db` (aio's tables and hash format, so the
//! file moves between aio and aio-rs both ways).
//!
//! - **Ids** are one key however they are typed: Unicode NFC and trimmed;
//!   invisible characters refused; unique by their NFKC case fold, so
//!   `ａｄｍｉｎ` (full width) or `Ａdmin` can't stand beside `admin`.
//! - **Passwords**: PBKDF2-HMAC-SHA-256 (`password`). An unknown id costs
//!   the same hash, so timing says nothing. 5 wrong in a row lock the
//!   account 15 minutes; a locked account is said only to the right
//!   password (423), so the lock is no oracle for guessers.
//! - **Sessions**: 256-bit tokens, stored hashed. TTL, sliding (a session
//!   used past half its life is renewed), revoke one or all, a per-user cap
//!   that evicts the oldest. The role is read live from the user, so a
//!   demotion counts at once. Every revocation and user change ticks
//!   [`Auth::changes`]: sync closes the sockets of sessions that died.
//! - **TOTP**: a code works once, even across a restart (the step used is
//!   kept with the user); wrong codes count toward the same lockout.
//! - A new password, or a removed user, revokes every session and burns
//!   every pending token.
//!
//! The HTTP doors (`/__aio/auth/*`, with per-address budgets) are served by
//! `aio::sync` when `Options::auth` is set.
pub mod budget;
#[cfg(feature = "server")]
pub mod client;
#[cfg(feature = "oidc")]
pub mod oidc;
/// Without `oidc` no identity provider can be set, so none is served.
#[cfg(all(feature = "server", not(feature = "oidc")))]
pub(crate) mod oidc {
    pub(crate) enum Provider {}
    pub(crate) type Page = (u16, Vec<(String, String)>, String);
}
pub mod password;
#[cfg(feature = "server")]
pub(crate) mod routes;
pub mod totp;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

/// Consecutive wrong passwords or codes that lock an account…
pub const LOCK_AFTER: i64 = 5;
/// …for this long.
pub const LOCK_MS: i64 = 15 * 60_000;
pub use aio_config::auth::{MAX_TTL_MS, Policy};
/// A TOTP login's second step must come within this long of the first.
pub const TOTP_PENDING_MS: i64 = 5 * 60_000;
/// A mailed email-verification token lives this long…
pub const VERIFY_MS: i64 = 24 * 3_600_000;
/// …and a password-reset one this long.
pub const RESET_MS: i64 = 15 * 60_000;
/// Ids in this namespace belong to identity providers: no signup may claim one.
pub const EXTERNAL_PREFIX: &str = "oidc:";

/// Why an account action was refused. `code()` is aio's wire code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    InvalidId,
    ReservedId,
    PasswordTooShort,
    UserExists,
    InvalidCredentials,
    Locked,
    InvalidCode,
    PendingExpired,
    TotpOn,
    SetupFirst,
    Storage(String),
}

impl Refusal {
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidId => "invalid_id",
            Self::ReservedId => "reserved_id",
            Self::PasswordTooShort => "password_too_short",
            Self::UserExists => "user_exists",
            Self::InvalidCredentials => "invalid_credentials",
            Self::Locked => "account_locked",
            Self::InvalidCode => "invalid_code",
            Self::PendingExpired => "pending_expired",
            Self::TotpOn => "totp_already_enabled",
            Self::SetupFirst => "setup_first",
            Self::Storage(_) => "auth_unavailable",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(e) => write!(f, "auth storage: {e}"),
            r => f.write_str(r.code()),
        }
    }
}

impl std::error::Error for Refusal {}

pub type Result<T, E = Refusal> = std::result::Result<T, E>;

/// A mail the login doors send (an email verification, a password reset),
/// handed to the app's hook (`Http::mail`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub text: String,
}

#[cfg(feature = "server")]
pub(crate) type Mailer = Arc<dyn Fn(&Mail) -> std::result::Result<(), String> + Send + Sync>;

/// A user as the app sees it: never the password hash or the TOTP secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub role: String,
    pub created_at: i64,
    pub email: Option<String>,
    pub verified: bool,
    pub totp: bool,
}

/// A live session: whose, their role now, and until when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub user: String,
    pub role: String,
    pub expires_at: i64,
}

/// Users the app knows by their token, without accounts (aio's `users` /
/// `resolveUser`): a service's callers, a fixed set of devices. A token
/// that resolves is that user with that role on every door (sockets,
/// routes, files, server fns); the shared key then opens none of them, so
/// every caller is someone.
#[derive(Clone)]
pub struct Users(Arc<Resolve>);

/// Token → (user, role).
type Resolve = dyn Fn(&str) -> Option<(String, String)> + Send + Sync;

impl Users {
    /// A fixed map, token → (user, role), compared in constant time. A
    /// token a guesser finds at network speed (`config::weak_token`) is said.
    pub fn map(tokens: impl IntoIterator<Item = (String, (String, String))>) -> Self {
        let tokens: Vec<(String, (String, String))> = tokens.into_iter().collect();
        for (_, (user, _)) in tokens.iter().filter(|(t, _)| crate::config::weak_token(t)) {
            eprintln!(
                "[aio] security: the token of user {user:?} is weak (under 16 characters or ~80 bits): it is \
                 guessable at network speed. Use 32 random characters (openssl rand -base64 24)"
            );
        }
        Self::resolve(move |shown| {
            // Every entry is compared, so the time says nothing of which matched.
            let ok = |t: &String| totp::same(t.as_bytes(), shown.as_bytes());
            tokens.iter().fold(None, |hit, (t, who)| if ok(t) && hit.is_none() { Some(who.clone()) } else { hit })
        })
    }

    /// The app's own resolver, token → (user, role); one that panics
    /// resolves no one.
    pub fn resolve(f: impl Fn(&str) -> Option<(String, String)> + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// Who `token` is, as a session that does not end.
    pub fn who(&self, token: &str) -> Option<Session> {
        if token.is_empty() {
            return None;
        }
        let (user, role) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.0)(token))).ok()??;
        Some(Session { user, role, expires_at: i64::MAX })
    }
}

/// What a correct password gives: a session, or (TOTP on) the token for the second step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Login {
    Signed { token: String, user: User },
    Totp { pending: String },
}

/// The accounts a running server serves; held weakly, so they go with it.
static SERVING: Mutex<Option<std::sync::Weak<Auth>>> = Mutex::new(None);

/// The accounts the running server signs its clients in with (aio
/// `serverAuth()`), for a server fn or route that manages them (an admin
/// screen); `None` with accounts off, before the server opens them and once
/// it has stopped.
pub fn serving() -> Option<Arc<Auth>> {
    SERVING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(std::sync::Weak::upgrade)
}

#[cfg(feature = "server")]
pub(crate) fn serve(auth: &Arc<Auth>) {
    *SERVING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(auth));
}

pub struct Auth {
    db: Mutex<Connection>,
    policy: Policy,
    /// Ticks on every revocation and user change (sockets re-check).
    changes: tokio::sync::watch::Sender<u64>,
    /// One password check at a time per account: a burst then counts
    /// exactly as the same attempts one by one.
    lanes: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// A test's clock; none: the wall clock.
    clock: Option<Arc<std::sync::atomic::AtomicI64>>,
}

fn lock<X>(m: &Mutex<X>) -> MutexGuard<'_, X> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn db<T>(r: rusqlite::Result<T>) -> Result<T> {
    r.map_err(|e| Refusal::Storage(e.to_string()))
}

pub(crate) fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).expect("the OS random source");
    b
}

/// A password to hand a person: 16 letters and digits, each drawn uniformly.
pub fn generated_password() -> String {
    const ABC: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::new();
    while out.len() < 16 {
        // 248 = 4 × 62: a byte past it would favour the first eight.
        out.extend(random::<32>().iter().filter(|b| **b < 248).map(|b| ABC[usize::from(b % 62)] as char));
    }
    out.truncate(16);
    out
}

pub(crate) use crate::sha256::hex;

/// How a token is kept: a stolen `auth.db` holds no usable one.
fn hashed(token: &str) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, token.as_bytes()).as_ref())
}

/// The one key of an id: NFC, trimmed.
pub fn normalize(id: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    id.nfc().collect::<String>().trim().to_string()
}

/// What two ids must not share (Unicode's NFKC_Casefold, by NFKC, lower
/// case, NFKC): compatibility forms (full width, ligatures, `ℌ`) and case
/// fold to one.
fn folded(id: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    id.nfkc().flat_map(char::to_lowercase).nfkc().collect()
}

/// Unicode's format characters (Cf): zero-width joiners, direction overrides…
fn is_format(c: char) -> bool {
    matches!(c as u32,
        0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2 | 0x180E | 0x200B..=0x200F
        | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD
        | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// Why a new account `id` with `password` can't be made, from the request
/// alone (no lookup, no hash: a refusal here costs nothing and says nothing).
pub fn policy_refusal(id: &str, password: &str) -> Option<Refusal> {
    let id = normalize(id);
    if id.is_empty()
        || id.chars().count() > 256
        || id.chars().any(|c| c.is_control() || c.is_whitespace() || is_format(c))
    {
        return Some(Refusal::InvalidId);
    }
    // Folded as far as uniqueness folds: `OIDC:` claims the namespace too.
    if folded(&id).starts_with(EXTERNAL_PREFIX) {
        return Some(Refusal::ReservedId);
    }
    (password.chars().count() < 8).then_some(Refusal::PasswordTooShort)
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS users (
  id TEXT PRIMARY KEY, pw TEXT NOT NULL, role TEXT NOT NULL, created_at INTEGER NOT NULL, email TEXT,
  verified INTEGER NOT NULL DEFAULT 0, totp TEXT, totp_on INTEGER NOT NULL DEFAULT 0,
  fails INTEGER NOT NULL DEFAULT 0, locked_until INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS sessions (
  token_hash TEXT PRIMARY KEY, user_id TEXT NOT NULL, role TEXT NOT NULL,
  created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS one_shot_tokens (
  token_hash TEXT PRIMARY KEY, kind TEXT NOT NULL, subject TEXT NOT NULL, payload TEXT, expires_at INTEGER NOT NULL
);";

const USER: &str = "SELECT id, role, created_at, email, verified, totp_on FROM users";

fn user_row(r: &rusqlite::Row) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        role: r.get(1)?,
        created_at: r.get(2)?,
        email: r.get(3)?,
        verified: r.get::<_, i64>(4)? == 1,
        totp: r.get::<_, i64>(5)? == 1,
    })
}

impl Auth {
    /// Open (or create) `auth.db` at `path`. A policy that can't hold is refused here.
    pub fn open(path: impl AsRef<Path>, policy: Policy) -> Result<Self> {
        Self::with(db(Connection::open(path))?, policy)
    }

    /// In memory (tests, a demo).
    pub fn memory(policy: Policy) -> Result<Self> {
        Self::with(db(Connection::open_in_memory())?, policy)
    }

    fn with(c: Connection, policy: Policy) -> Result<Self> {
        if !(1..=MAX_TTL_MS).contains(&policy.ttl_ms) {
            return Err(Refusal::Storage(format!("auth: ttl_ms {} is not 1 ms to a century", policy.ttl_ms)));
        }
        if policy.max_sessions == Some(0) {
            return Err(Refusal::Storage("auth: max_sessions must be at least 1".into()));
        }
        // `ax auth` and the app both write it: wait for the other's write.
        db(c.busy_timeout(std::time::Duration::from_secs(1)))?;
        db(c.pragma_update(None, "journal_mode", "WAL"))?;
        db(c.execute_batch(SCHEMA))?;
        let has_step: bool = db(c.query_row(
            "SELECT EXISTS (SELECT 1 FROM pragma_table_info('users') WHERE name = 'totp_step')",
            [],
            |r| r.get(0),
        ))?;
        if !has_step {
            db(c.execute_batch("ALTER TABLE users ADD COLUMN totp_step INTEGER NOT NULL DEFAULT 0"))?;
        }
        let a = Self {
            db: Mutex::new(c),
            policy,
            changes: tokio::sync::watch::channel(0).0,
            lanes: Mutex::default(),
            clock: None,
        };
        let now = a.now();
        let c = lock(&a.db);
        db(c.execute("DELETE FROM sessions WHERE expires_at <= ?1", [now]))?;
        // A cap set or lowered while the app was off holds from the start.
        if let Some(max) = a.policy.max_sessions {
            let gone = db(c.execute(
                "DELETE FROM sessions WHERE rowid IN (SELECT rowid FROM (SELECT rowid, ROW_NUMBER() OVER \
                 (PARTITION BY user_id ORDER BY rowid DESC) AS n FROM sessions) WHERE n > ?1)",
                [max],
            ))?;
            if gone > 0 {
                eprintln!("[aio] auth: {gone} session(s) over max_sessions ({max}) ended; each user keeps the newest");
            }
        }
        drop(c);
        Ok(a)
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Ticks after every revocation and user change.
    pub fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    fn changed(&self) {
        self.changes.send_modify(|n| *n += 1);
    }

    fn now(&self) -> i64 {
        self.clock.as_ref().map_or_else(crate::store::now_ms, |c| c.load(std::sync::atomic::Ordering::Relaxed))
    }

    #[cfg(test)]
    fn on_clock(mut self, clock: Arc<std::sync::atomic::AtomicI64>) -> Self {
        self.clock = Some(clock);
        self
    }

    // ── users ──────────────────────────────────────────────────────────

    /// A new account (role `"user"` unless given).
    pub fn create(&self, id: &str, password: &str, role: Option<&str>) -> Result<User> {
        if let Some(r) = policy_refusal(id, password) {
            return Err(r);
        }
        let id = normalize(id);
        let pw = password::hash(password);
        let c = lock(&self.db);
        let mine = folded(&id);
        let mut ids = db(c.prepare("SELECT id FROM users"))?;
        let taken = db(ids.query_map([], |r| r.get::<_, String>(0)))?.flatten().any(|o| folded(&o) == mine);
        drop(ids);
        if taken {
            return Err(Refusal::UserExists);
        }
        let (role, at) = (role.unwrap_or("user"), self.now());
        db(c.execute(
            "INSERT INTO users (id, pw, role, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, pw, role, at],
        ))?;
        Ok(User { id, role: role.into(), created_at: at, email: None, verified: false, totp: false })
    }

    /// An identity provider's account (`oidc:…`, as `oidc::account_id`
    /// spells it; the operator seeds one to admit it with sign-up off): its
    /// own namespace, exact (a `sub` is case-sensitive), no password that
    /// could ever match; `email` is the provider's verified one.
    pub fn create_external(&self, id: &str, role: &str, email: Option<&str>) -> Result<User> {
        if !id.starts_with(EXTERNAL_PREFIX) || id.chars().count() > 256 || id.chars().any(char::is_control) {
            return Err(Refusal::InvalidId);
        }
        let at = self.now();
        let n = db(lock(&self.db).execute(
            "INSERT OR IGNORE INTO users (id, pw, role, created_at, email, verified) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, password::decoy(), role, at, email, email.is_some()],
        ))?;
        if n == 0 {
            return Err(Refusal::UserExists);
        }
        self.changed();
        Ok(User {
            id: id.into(),
            role: role.into(),
            created_at: at,
            email: email.map(Into::into),
            verified: email.is_some(),
            totp: false,
        })
    }

    pub fn user(&self, id: &str) -> Result<Option<User>> {
        let c = lock(&self.db);
        db(c.query_row(&format!("{USER} WHERE id = ?1"), [normalize(id)], user_row).optional())
    }

    /// Every user, newest first.
    pub fn users(&self) -> Result<Vec<User>> {
        let c = lock(&self.db);
        let mut q = db(c.prepare(&format!("{USER} ORDER BY created_at DESC, rowid DESC")))?;
        db(db(q.query_map([], user_row))?.collect())
    }

    /// Check a password: the user, `Locked` (only for the right password),
    /// or `InvalidCredentials`.
    pub fn verify(&self, id: &str, password: &str) -> Result<User> {
        let id = normalize(id);
        let lane = lock(&self.lanes).entry(id.clone()).or_default().clone();
        let r = {
            let _one = lock(&lane);
            self.verify_now(&id, password)
        };
        let mut lanes = lock(&self.lanes);
        if lanes.get(&id).is_some_and(|l| Arc::strong_count(l) <= 2) {
            lanes.remove(&id);
        }
        r
    }

    fn verify_now(&self, id: &str, password: &str) -> Result<User> {
        let row = |c: &Connection| {
            c.query_row("SELECT pw FROM users WHERE id = ?1", [id], |r| r.get::<_, String>(0)).optional()
        };
        let pw = db(row(&lock(&self.db)))?;
        let ok = password::verify(password, pw.as_deref().unwrap_or(&password::decoy()));
        let Some(pw) = pw.filter(|_| !id.starts_with(EXTERNAL_PREFIX)) else {
            return Err(Refusal::InvalidCredentials);
        };
        if !ok {
            self.fail(id, "passwords")?;
            return Err(Refusal::InvalidCredentials);
        }
        let c = lock(&self.db);
        // Changed or removed while it was hashing: not this account any more.
        if db(row(&c))?.as_deref() != Some(&pw) {
            return Err(Refusal::InvalidCredentials);
        }
        let (locked_until, totp_on): (i64, i64) =
            db(c.query_row("SELECT locked_until, totp_on FROM users WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            }))?;
        if locked_until > self.now() {
            return Err(Refusal::Locked);
        }
        // With a second factor, the password alone doesn't reset the count.
        if totp_on == 0 {
            db(c.execute("UPDATE users SET fails = 0, locked_until = 0 WHERE id = ?1", [id]))?;
        }
        db(c.query_row(&format!("{USER} WHERE id = ?1"), [id], user_row))
    }

    /// Count one wrong password or code; true when the account is locked after it.
    fn fail(&self, id: &str, what: &str) -> Result<bool> {
        let now = self.now();
        let c = lock(&self.db);
        let n = db(c.execute(
            "UPDATE users SET fails = CASE WHEN fails + 1 >= ?1 THEN 0 ELSE fails + 1 END, \
             locked_until = CASE WHEN fails + 1 >= ?1 THEN ?2 ELSE 0 END WHERE id = ?3 AND locked_until <= ?4",
            params![LOCK_AFTER, now + LOCK_MS, id, now],
        ))?;
        let until: i64 =
            db(c.query_row("SELECT locked_until FROM users WHERE id = ?1", [id], |r| r.get(0)).optional())?
                .unwrap_or(0);
        if until <= now {
            return Ok(false);
        }
        if n == 1 {
            eprintln!(
                "[aio] auth: account \"{id}\" locked for {} min after {LOCK_AFTER} wrong {what}",
                LOCK_MS / 60_000
            );
        }
        // Second steps already in flight can't keep guessing.
        db(c.execute("DELETE FROM one_shot_tokens WHERE subject = ?1 AND kind = 'totp'", [id]))?;
        Ok(true)
    }

    /// End a lockout (the operator's rescue).
    pub fn unlock(&self, id: &str) -> Result<bool> {
        self.update("UPDATE users SET fails = 0, locked_until = 0 WHERE id = ?2", None, id)
    }

    pub fn set_role(&self, id: &str, role: &str) -> Result<bool> {
        self.update("UPDATE users SET role = ?1 WHERE id = ?2", Some(role), id)
    }

    /// A new address is unverified until confirmed.
    pub fn set_email(&self, id: &str, email: &str) -> Result<bool> {
        self.update(
            "UPDATE users SET verified = CASE WHEN email IS ?1 THEN verified ELSE 0 END, email = ?1 WHERE id = ?2",
            Some(email),
            id,
        )
    }

    pub fn mark_verified(&self, id: &str) -> Result<bool> {
        self.update("UPDATE users SET verified = 1 WHERE id = ?2", None, id)
    }

    fn update(&self, sql: &str, v: Option<&str>, id: &str) -> Result<bool> {
        let id = normalize(id);
        let c = lock(&self.db);
        // Every statement here takes the value as ?1 (or none) and the id as ?2.
        let ok = db(c.execute(sql, params![v, id]))? > 0;
        drop(c);
        if ok {
            self.changed();
        }
        Ok(ok)
    }

    /// A new password: also ends any lockout, burns every pending token and
    /// revokes every session (an account rescued is rescued from everyone).
    pub fn set_password(&self, id: &str, password: &str) -> Result<bool> {
        if password.chars().count() < 8 {
            return Err(Refusal::PasswordTooShort);
        }
        let (id, pw) = (normalize(id), password::hash(password));
        let n = db(lock(&self.db).execute(
            "UPDATE users SET pw = ?1, fails = 0, locked_until = 0, \
             totp = CASE WHEN totp_on = 1 THEN totp ELSE NULL END WHERE id = ?2",
            params![pw, id],
        ))?;
        if n == 0 {
            return Ok(false);
        }
        self.end_all(&id)?;
        Ok(true)
    }

    /// Remove an account, its sessions and its pending tokens.
    pub fn remove(&self, id: &str) -> Result<bool> {
        let id = normalize(id);
        if db(lock(&self.db).execute("DELETE FROM users WHERE id = ?1", [&id]))? == 0 {
            return Ok(false);
        }
        self.end_all(&id)?;
        Ok(true)
    }

    /// End everything `id` holds: its sessions (how many) and its pending
    /// tokens (password resets, second-factor steps).
    pub fn end_all(&self, id: &str) -> Result<usize> {
        let id = normalize(id);
        db(lock(&self.db).execute("DELETE FROM one_shot_tokens WHERE subject = ?1 AND kind != 'oidc'", [&id]))?;
        let n = self.revoke_user(&id)?;
        self.changed();
        Ok(n)
    }

    // ── logins ─────────────────────────────────────────────────────────

    /// The password step: a session, or the pending token for the TOTP step.
    pub fn login(&self, id: &str, password: &str) -> Result<Login> {
        self.signed_in(self.verify(id, password)?)
    }

    /// After a right password: a session, or the pending token for the TOTP step.
    pub(crate) fn signed_in(&self, user: User) -> Result<Login> {
        if user.totp {
            return Ok(Login::Totp { pending: self.one_shot("totp", &user.id, TOTP_PENDING_MS)? });
        }
        Ok(Login::Signed { token: self.issue(&user.id, &user.role)?, user })
    }

    /// The TOTP step. A pending token is spent by any answer; a wrong code
    /// counts toward the lockout.
    pub fn login_totp(&self, pending: &str, code: &str) -> Result<(String, User)> {
        let id = self.consume("totp", pending)?.ok_or(Refusal::PendingExpired)?;
        let locked: i64 = db(lock(&self.db)
            .query_row("SELECT locked_until FROM users WHERE id = ?1", [&id], |r| r.get(0))
            .optional())?
        .unwrap_or(0);
        if locked > self.now() {
            return Err(Refusal::Locked);
        }
        if !self.spend_code(&id, code, true)? {
            return Err(if self.fail(&id, "codes")? { Refusal::Locked } else { Refusal::InvalidCode });
        }
        db(lock(&self.db).execute("UPDATE users SET fails = 0, locked_until = 0 WHERE id = ?1", [&id]))?;
        let user = self.user(&id)?.ok_or(Refusal::InvalidCode)?;
        Ok((self.issue(&user.id, &user.role)?, user))
    }

    /// Is `code` an unspent code of the user's secret (`enabled`: the active
    /// one, else the staged one)? Spends it: one atomic step forward.
    fn spend_code(&self, id: &str, code: &str, enabled: bool) -> Result<bool> {
        let c = lock(&self.db);
        let row: Option<(Option<String>, i64, i64)> = db(c
            .query_row("SELECT totp, totp_on, totp_step FROM users WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional())?;
        let Some((Some(secret), _, used)) = row.filter(|r| (r.1 == 1) == enabled) else { return Ok(false) };
        let Some(step) = totp::unbase32(&secret).and_then(|k| totp::accept(&k, code, self.now(), used)) else {
            return Ok(false);
        };
        Ok(db(c.execute("UPDATE users SET totp_step = ?1 WHERE id = ?2 AND totp_step < ?1", params![step, id]))? == 1)
    }

    /// Stage a fresh TOTP secret for `id` (shown once, to enrol); it counts
    /// only after `totp_enable`.
    pub fn totp_setup(&self, id: &str) -> Result<String> {
        let (id, secret) = (normalize(id), totp::secret());
        let c = lock(&self.db);
        let on: Option<i64> =
            db(c.query_row("SELECT totp_on FROM users WHERE id = ?1", [&id], |r| r.get(0)).optional())?;
        match on {
            None => Err(Refusal::InvalidCredentials),
            Some(1) => Err(Refusal::TotpOn),
            Some(_) => {
                // A new secret starts its own count of used steps.
                db(c.execute("UPDATE users SET totp = ?1, totp_step = 0 WHERE id = ?2", params![secret, id]))?;
                Ok(secret)
            }
        }
    }

    /// Turn the staged secret on, proven by one of its codes.
    pub fn totp_enable(&self, id: &str, code: &str) -> Result<()> {
        let id = normalize(id);
        let staged: Option<String> =
            db(lock(&self.db).query_row("SELECT totp FROM users WHERE id = ?1", [&id], |r| r.get(0)).optional())?
                .flatten();
        if staged.is_none() {
            return Err(Refusal::SetupFirst);
        }
        if !self.spend_code(&id, code, false)? {
            return Err(Refusal::InvalidCode);
        }
        db(lock(&self.db).execute("UPDATE users SET totp_on = 1 WHERE id = ?1 AND totp IS NOT NULL", [&id]))?;
        self.changed();
        Ok(())
    }

    /// Clear the second factor (a lost phone: the user with their password,
    /// or the operator). True when one was set.
    pub fn totp_off(&self, id: &str) -> Result<bool> {
        self.update(
            "UPDATE users SET totp_on = 0, totp = NULL WHERE id = ?2 AND (totp IS NOT NULL OR totp_on = 1)",
            None,
            id,
        )
    }

    fn one_shot(&self, kind: &str, subject: &str, ttl_ms: i64) -> Result<String> {
        self.one_shot_with(kind, subject, ttl_ms, None)
    }

    /// [`Self::one_shot`] carrying `payload` back to whoever spends it.
    fn one_shot_with(&self, kind: &str, subject: &str, ttl_ms: i64, payload: Option<&str>) -> Result<String> {
        let token = format!("aiot_{}", hex(&random::<32>()));
        let (c, now) = (lock(&self.db), self.now());
        db(c.execute("DELETE FROM one_shot_tokens WHERE expires_at <= ?1", [now]))?;
        db(c.execute(
            "INSERT INTO one_shot_tokens (token_hash, kind, subject, payload, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![hashed(&token), kind, subject, payload, now + ttl_ms],
        ))?;
        Ok(token)
    }

    /// Spend a one-shot token: its subject, or `None` (unknown, expired, spent).
    fn consume(&self, kind: &str, token: &str) -> Result<Option<String>> {
        Ok(self.consume_with(kind, token)?.map(|r| r.0))
    }

    /// [`Self::consume`], with what it carried.
    fn consume_with(&self, kind: &str, token: &str) -> Result<Option<(String, Option<String>)>> {
        let (c, h) = (lock(&self.db), hashed(token));
        let row: Option<(String, Option<String>, i64)> = db(c
            .query_row(
                "SELECT subject, payload, expires_at FROM one_shot_tokens WHERE token_hash = ?1 AND kind = ?2",
                params![h, kind],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional())?;
        db(c.execute("DELETE FROM one_shot_tokens WHERE token_hash = ?1", [&h]))?;
        Ok(row.filter(|r| r.2 > self.now()).map(|r| (r.0, r.1)))
    }

    // ── sessions ───────────────────────────────────────────────────────

    /// A new session for `user` (the token is shown once: only its hash is kept).
    pub fn issue(&self, user: &str, role: &str) -> Result<String> {
        let token = format!("aios_{}", hex(&random::<32>()));
        let (c, now) = (lock(&self.db), self.now());
        db(c.execute(
            "INSERT INTO sessions (token_hash, user_id, role, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![hashed(&token), user, role, now, now + self.policy.ttl_ms],
        ))?;
        let Some(max) = self.policy.max_sessions else { return Ok(token) };
        let gone = db(c.execute(
            "DELETE FROM sessions WHERE user_id = ?1 AND expires_at > ?2 AND token_hash NOT IN (SELECT token_hash \
             FROM sessions WHERE user_id = ?1 AND expires_at > ?2 ORDER BY rowid DESC LIMIT ?3)",
            params![user, now, max],
        ))?;
        drop(c);
        if gone > 0 {
            eprintln!("[aio] auth: user {user} is over max_sessions ({max}): {gone} oldest session(s) ended");
            self.changed();
        }
        Ok(token)
    }

    /// The live session of `token`, with its user's role now; `None` when
    /// unknown, expired or revoked. One used past half its life is renewed.
    pub fn session(&self, token: &str) -> Result<Option<Session>> {
        let (c, h, now) = (lock(&self.db), hashed(token), self.now());
        let row: Option<(String, String, i64, Option<String>)> = db(c
            .query_row(
                "SELECT s.user_id, s.role, s.expires_at, u.role FROM sessions s LEFT JOIN users u ON u.id = s.user_id \
                 WHERE s.token_hash = ?1",
                [&h],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional())?;
        let Some((user, role, mut expires_at, live)) = row else { return Ok(None) };
        if expires_at <= now {
            db(c.execute("DELETE FROM sessions WHERE token_hash = ?1", [&h]))?;
            return Ok(None);
        }
        if expires_at - now < self.policy.ttl_ms / 2 {
            expires_at = now + self.policy.ttl_ms;
            db(c.execute("UPDATE sessions SET expires_at = ?1 WHERE token_hash = ?2", params![expires_at, h]))?;
        }
        Ok(Some(Session { user, role: live.unwrap_or(role), expires_at }))
    }

    /// Renew a live session to a full TTL; false when it isn't live.
    pub fn refresh(&self, token: &str) -> Result<bool> {
        let now = self.now();
        let n = db(lock(&self.db).execute(
            "UPDATE sessions SET expires_at = ?1 WHERE token_hash = ?2 AND expires_at > ?3",
            params![now + self.policy.ttl_ms, hashed(token), now],
        ))?;
        Ok(n > 0)
    }

    /// End one session (logout).
    pub fn revoke(&self, token: &str) -> Result<bool> {
        let n = db(lock(&self.db).execute("DELETE FROM sessions WHERE token_hash = ?1", [hashed(token)]))?;
        if n > 0 {
            self.changed();
        }
        Ok(n > 0)
    }

    /// End every session of `user` (a kick, a breach); how many.
    pub fn revoke_user(&self, user: &str) -> Result<usize> {
        let n = db(lock(&self.db).execute("DELETE FROM sessions WHERE user_id = ?1", [normalize(user)]))?;
        if n > 0 {
            eprintln!("[aio] auth: all {n} session(s) of {user} ended");
            self.changed();
        }
        Ok(n)
    }

    /// Live sessions.
    pub fn sessions(&self) -> Result<usize> {
        let c = lock(&self.db);
        db(c.execute("DELETE FROM sessions WHERE expires_at <= ?1", [self.now()]))?;
        db(c.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))).map(|n| n as usize)
    }
}

#[cfg(test)]
mod tests;

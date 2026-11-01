//! A client for the login doors (`aio::auth` routes, aio's `createAuthClient`):
//! a native app, a tool or a test signs in to an app, then links with the
//! session as its key ([`Client::link`] → `Remote::connect_to`). Over TLS
//! to a pinned root when it has one, as [`crate::sync::Link`].
//!
//! In a page (`wasm32`) it asks the server that served the page, its
//! session kept in the HttpOnly cookie only ([`SESSION_HEADER`]).
//!
//! A refusal is an [`AuthError`]: the server's code to branch on, and a
//! sentence a person can act on as its `Display` (aio's wording).
use serde_json::{Value, json};

/// Asked as `cookie`: the session stays in the HttpOnly cookie the door
/// sets, and the answer carries no token for the page's script to hold.
pub const SESSION_HEADER: &str = "x-aio-session";

/// A signed-in session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    pub user: String,
    pub role: String,
    /// The session: shown as `Authorization: Bearer`, and as a link's key.
    /// Empty in a page: there it is the cookie's.
    pub token: String,
}

/// What a login answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Login {
    Signed(Signed),
    /// The account has a second factor: [`Client::totp`] with `pending`
    /// within [`super::TOTP_PENDING_MS`].
    Totp {
        pending: String,
    },
}

/// What a sign-up answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signup {
    Signed(Signed),
    /// The app needs the email verified first: no session yet. `sent`:
    /// the mail went out (else [`Client::verify_resend`] tries again).
    Verify {
        sent: bool,
    },
}

/// What the server offers (`GET me`): who is signed in, and its doors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Me {
    /// `(id, role)` of the session shown, if it is live.
    pub user: Option<(String, String)>,
    /// Anyone may create an account.
    pub signup: bool,
    pub totp: bool,
    /// It sends mail: email verification and password resets work.
    pub mail: bool,
}

/// A door's refusal: `code` as the server says it (`invalid_credentials`),
/// `http_<status>` when it said none, `unreachable` (status 0) when no
/// answer came.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthError {
    pub code: String,
    pub status: u16,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&text(&self.code, self.status))
    }
}

impl std::error::Error for AuthError {}

/// The sentence for a server `code`, once for every client (aio's table);
/// an unknown one is said with its code rather than as a wrong sentence.
pub fn text(code: &str, status: u16) -> String {
    let s = match code {
        "invalid_credentials" => "Incorrect username or password.",
        "account_locked" => {
            "This account is locked after too many failed attempts. Try again later or ask an administrator to \
             unlock it."
        }
        "too_many_attempts" => "Too many attempts. Wait a moment and try again.",
        "login_required" => "You are not signed in.",
        "signup_disabled" => "This app does not accept new sign-ups.",
        "auth_unavailable" | "auth_failed" => {
            "Sign-in is unavailable right now — the server could not reach its account store. Try again shortly."
        }
        "password_too_short" => "Password must be at least 8 characters.",
        "invalid_id" => "That username cannot be used — pick another.",
        "reserved_id" => {
            "That username is reserved for accounts that sign in through an external provider — pick another."
        }
        "too_many_accounts" => "Too many accounts were created from this address. Try again later.",
        "body_too_large" => "That request was too large.",
        "user_exists" => "That username is already taken.",
        "password_required" => "A password is required.",
        "id_and_password_required" => "Enter both a username and a password.",
        "old_and_new_required" => "Enter your current password and the new one.",
        "invalid_code" => "That code is not correct.",
        "pending_and_code_required" => "Enter the 6-digit code from your authenticator app.",
        "pending_expired" => "The sign-in took too long. Start again.",
        "setup_first" => "Set up two-factor authentication before confirming it.",
        "totp_already_enabled" => {
            "Two-factor authentication is already on. Turn it off first (that needs your password), then set it up \
             again."
        }
        "unknown_auth_route" => "That auth endpoint does not exist.",
        "email_unverified" => {
            "This account's email address has not been verified yet. Check your inbox for the verification link."
        }
        "invalid_email" => "That email address is not valid.",
        "email_required" => "An email address is required.",
        "token_required" => "The link is incomplete — request a new one.",
        "token_and_password_required" => "The reset link is incomplete — request a new one.",
        "invalid_or_expired_token" => "That link has expired. Request a new one.",
        "no_email_on_account" => "This account has no email address, so it cannot be reset by email.",
        "external_identity" => {
            "This account signs in through an external provider — use that provider's button rather than a password."
        }
        "mail_not_configured" => {
            "This app cannot send email, so it cannot verify addresses or reset passwords. (Server: no mail hook, \
             Http::mail, is set.)"
        }
        "mail_failed" => "The email could not be sent. Try again later.",
        "unreachable" => "The app can't be reached. Check the connection and try again.",
        c if c.starts_with("http_") => return format!("The auth server answered {status} with no reason given."),
        c => return format!("Sign-in failed ({c})."),
    };
    s.into()
}

/// The login doors of the app at `addr` (`host:port`).
#[derive(Clone)]
pub struct Client {
    pub addr: String,
    /// The app's root (PEM) to pin: then TLS, else plain.
    pub root: Option<String>,
    via: Option<Via>,
}

/// Another way to the doors ([`Client::via`]): `(path, session, body)` →
/// the status and the JSON answered.
pub type Via = std::sync::Arc<
    dyn Fn(
            String,
            Option<String>,
            Option<Value>,
        ) -> std::pin::Pin<Box<dyn Future<Output = std::io::Result<(u16, Value)>> + Send>>
        + Send
        + Sync,
>;

impl PartialEq for Client {
    fn eq(&self, o: &Self) -> bool {
        let via = match (&self.via, &o.via) {
            (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        };
        self.addr == o.addr && self.root == o.root && via
    }
}

impl Eq for Client {}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("addr", &self.addr).field("root", &self.root).finish_non_exhaustive()
    }
}

type Answer = Result<Value, AuthError>;

impl Client {
    pub fn new(addr: impl Into<String>, root: Option<String>) -> Self {
        Self { addr: addr.into(), root, via: None }
    }

    /// Doors reached through `via` instead of a socket: a browser tab's
    /// page asks them itself (`aio_shell::web::client`), so the session
    /// cookie lands in the browser and each sign-in counts for its address.
    pub fn via(via: Via) -> Self {
        Self { addr: String::new(), root: None, via: Some(via) }
    }

    /// The link a signed-in copy connects over, with `s` as its key.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn link(&self, s: &Signed) -> crate::sync::Link {
        crate::sync::Link { addr: self.addr.clone(), token: s.token.clone(), root: self.root.clone() }
    }

    /// Who `token` is (`None`: no live session), and what the server offers.
    pub async fn me(&self, token: Option<&str>) -> Result<Me, AuthError> {
        let v = self.ask("me", token, None).await?;
        let s = |k: &str| v["user"][k].as_str().map(String::from);
        let f = |k: &str| v["features"][k].as_bool().unwrap_or(false);
        Ok(Me { user: s("id").zip(s("role")), signup: f("signup"), totp: f("totp"), mail: f("mail") })
    }

    /// Create an account (when the app allows it), with an `email` to verify.
    pub async fn signup(&self, id: &str, password: &str, email: Option<&str>) -> Result<Signup, AuthError> {
        let v = self.ask("signup", None, Some(json!({ "id": id, "password": password, "email": email }))).await?;
        match v["verificationSent"].as_bool() {
            Some(sent) => Ok(Signup::Verify { sent }),
            None => signed(v).map(Signup::Signed),
        }
    }

    pub async fn login(&self, id: &str, password: &str) -> Result<Login, AuthError> {
        let v = self.ask("login", None, Some(json!({ "id": id, "password": password }))).await?;
        match v["pending"].as_str() {
            Some(p) if v["totpRequired"] == true => Ok(Login::Totp { pending: p.into() }),
            _ => signed(v).map(Login::Signed),
        }
    }

    /// A login's second step. A wrong code spends `pending`: start again.
    pub async fn totp(&self, pending: &str, code: &str) -> Result<Signed, AuthError> {
        signed(self.ask("totp", None, Some(json!({ "pending": pending, "code": code }))).await?)
    }

    /// End session `token`.
    pub async fn logout(&self, token: &str) -> Result<(), AuthError> {
        self.ask("logout", Some(token), Some(json!({}))).await.map(drop)
    }

    /// A new password: every session ends, and this one gets a new one.
    pub async fn password(&self, token: &str, old: &str, new: &str) -> Result<Signed, AuthError> {
        signed(self.ask("password", Some(token), Some(json!({ "old": old, "new": new }))).await?)
    }

    /// Begin a second factor: its secret and `otpauth://` URI (a QR code).
    pub async fn totp_setup(&self, token: &str) -> Result<(String, String), AuthError> {
        let v = self.ask("totp/setup", Some(token), Some(json!({}))).await?;
        let s = |k: &str| v[k].as_str().map(String::from);
        s("secret").zip(s("uri")).ok_or_else(|| AuthError { code: "http_200".into(), status: 200 })
    }

    /// Turn the factor on with a `code` from it; it takes the password too.
    pub async fn totp_enable(&self, token: &str, code: &str, password: &str) -> Result<(), AuthError> {
        self.ask("totp/enable", Some(token), Some(json!({ "code": code, "password": password }))).await.map(drop)
    }

    pub async fn totp_disable(&self, token: &str, password: &str) -> Result<(), AuthError> {
        self.ask("totp/disable", Some(token), Some(json!({ "password": password }))).await.map(drop)
    }

    /// Mail a new verification token to the signed-in account.
    pub async fn verify_request(&self, token: &str) -> Result<(), AuthError> {
        self.ask("verify/request", Some(token), Some(json!({}))).await.map(drop)
    }

    /// Mail a new verification token to account `id`, if it waits for one;
    /// answers the same either way.
    pub async fn verify_resend(&self, id: &str) -> Result<(), AuthError> {
        self.ask("verify/resend", None, Some(json!({ "id": id }))).await.map(drop)
    }

    /// Prove the email with the mailed `token`.
    pub async fn verify(&self, token: &str) -> Result<(), AuthError> {
        self.ask("verify", None, Some(json!({ "token": token }))).await.map(drop)
    }

    /// Mail account `id` a password-reset token; answers the same whether or
    /// not it exists.
    pub async fn request_reset(&self, id: &str) -> Result<(), AuthError> {
        self.ask("reset/request", None, Some(json!({ "id": id }))).await.map(drop)
    }

    /// Set a new password with the mailed `token`: every session ends.
    pub async fn reset(&self, token: &str, password: &str) -> Result<(), AuthError> {
        self.ask("reset", None, Some(json!({ "token": token, "password": password }))).await.map(drop)
    }

    /// `POST` `body` to door `op` (a `GET` without one).
    async fn ask(&self, op: &str, token: Option<&str>, body: Option<Value>) -> Answer {
        let gone = |_| AuthError { code: "unreachable".into(), status: 0 };
        let (status, v) = self.exchange(&format!("/__aio/auth/{op}"), token, body.as_ref()).await.map_err(gone)?;
        match status {
            200..300 => Ok(v),
            _ => Err(AuthError { code: v["error"].as_str().map_or(format!("http_{status}"), String::from), status }),
        }
    }

    #[cfg(target_arch = "wasm32")]
    async fn exchange(&self, path: &str, token: Option<&str>, body: Option<&Value>) -> std::io::Result<(u16, Value)> {
        match &self.via {
            Some(via) => via(path.into(), token.map(Into::into), body.cloned()).await,
            None => crate::web::fetch(path, token, body).await,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn exchange(&self, path: &str, token: Option<&str>, body: Option<&Value>) -> std::io::Result<(u16, Value)> {
        if let Some(via) = &self.via {
            return via(path.into(), token.map(Into::into), body.cloned()).await;
        }
        let host = self.addr.rsplit_once(':').map_or(self.addr.as_str(), |(h, _)| h).trim_matches(['[', ']']);
        crate::capabilities::check("sign-in", &self.addr)?;
        let tcp = tokio::net::TcpStream::connect(&self.addr).await?;
        let s: Box<dyn crate::http::Io> = match &self.root {
            #[cfg(not(feature = "tls"))]
            Some(_) => return Err(crate::left_out("tls", "a link pinned to a root")),
            #[cfg(feature = "tls")]
            Some(pem) => {
                let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(std::io::Error::other)?;
                Box::new(tokio_rustls::TlsConnector::from(crate::tls::client(pem)?).connect(name, tcp).await?)
            }
            None => Box::new(tcp),
        };
        crate::http::request(s, host, path, token, body).await
    }
}

fn signed(v: Value) -> Result<Signed, AuthError> {
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).map(String::from);
    // A page's session is its cookie's.
    let token = s("/token").or_else(|| cfg!(target_arch = "wasm32").then(String::new));
    match (s("/user/id"), s("/user/role"), token) {
        (Some(user), Some(role), Some(token)) => Ok(Signed { user, role, token }),
        _ => Err(AuthError { code: "http_200".into(), status: 200 }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_the_doors_send_has_a_sentence() {
        let src = include_str!("routes.rs");
        // Each `err(<status>, "<code>")` the doors answer.
        let said = |s: &'static str| s.split_once(", \"").filter(|(n, _)| n.bytes().all(|b| b.is_ascii_digit()));
        let mut codes: Vec<&str> =
            src.split("err(").skip(1).filter_map(said).filter_map(|(_, r)| r.split('"').next()).collect();
        use super::super::Refusal as R;
        let store = [
            R::InvalidId,
            R::ReservedId,
            R::PasswordTooShort,
            R::UserExists,
            R::InvalidCredentials,
            R::Locked,
            R::InvalidCode,
            R::PendingExpired,
            R::TotpOn,
            R::SetupFirst,
            R::Storage(String::new()),
        ];
        codes.extend(store.iter().map(R::code));
        assert!(codes.len() > 20, "the scan broke: {codes:?}");
        let untold: Vec<_> = codes.iter().filter(|c| text(c, 400).contains(**c)).collect();
        assert!(untold.is_empty(), "codes with no sentence: {untold:?}");
        assert_eq!(text("new_code", 400), "Sign-in failed (new_code).");
        assert_eq!(text("http_502", 502), "The auth server answered 502 with no reason given.");
    }
}

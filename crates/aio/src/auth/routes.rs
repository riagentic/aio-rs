//! The login doors, at `/__aio/auth/*` (aio's routes and wire codes):
//! `GET me`; `POST signup`, `login`, `totp` (the second step), `logout`,
//! `password`, `totp/setup`, `totp/enable`, `totp/disable`; with the app's
//! mail hook, `verify/request`, `verify/resend`, `verify`, `reset/request`,
//! `reset`. The anonymous mail doors answer 200 whether or not the account
//! exists, with the same work either way (a decoy token on a miss) and the
//! mail sent off the answer's path, so they tell a prober nothing; each is
//! budgeted per address and per account (an inbox is no mail-bomb target).
//! A session is
//! shown as `Authorization: Bearer`, or by a browser in the HttpOnly,
//! SameSite=Strict cookie a sign-in sets (`Secure` over TLS, or behind a
//! trusted proxy that says https); never in a URL. Every answer is
//! `no-store`. Password checks are budgeted per address (`budget`): work
//! first (a right password gets its unit back), then failures, which only
//! decide what a wrong one is answered; a right one is always served.
use super::budget::{self, Budgets};
use super::{Auth, Login, Refusal, User};
use serde_json::{Value, json};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

const MAX_BODY: usize = 16 * 1024;

pub(crate) struct Doors {
    pub auth: Arc<Auth>,
    pub title: String,
    budgets: Mutex<Budgets>,
    /// Served over TLS: the session cookie is `Secure`.
    secure: bool,
    /// The trusted proxy's header ([`crate::gate`]).
    trust: Option<String>,
    /// The app's mail hook: the verify and reset doors need it.
    mail: Option<super::Mailer>,
    /// The identity provider (`oidc/start`, `oidc/callback`).
    oidc: Option<Arc<super::oidc::Provider>>,
}

type Answer = (u16, Value);

fn err(status: u16, code: &str) -> Answer {
    (status, json!({ "error": code }))
}

fn refused(r: &Refusal) -> Answer {
    let status = match r {
        Refusal::InvalidId | Refusal::ReservedId | Refusal::PasswordTooShort | Refusal::SetupFirst => 400,
        Refusal::InvalidCredentials | Refusal::InvalidCode | Refusal::PendingExpired => 401,
        Refusal::UserExists | Refusal::TotpOn => 409,
        Refusal::Locked => 423,
        Refusal::Storage(e) => {
            eprintln!("[aio] auth: {e}");
            503
        }
    };
    err(status, r.code())
}

fn signed(token: String, u: &User, status: u16) -> Answer {
    (status, json!({ "user": { "id": u.id, "role": u.role }, "token": token }))
}

impl Doors {
    pub(crate) fn new(
        auth: Arc<Auth>,
        title: &str,
        secure: bool,
        trust: Option<String>,
        mail: Option<super::Mailer>,
        oidc: Option<Arc<super::oidc::Provider>>,
    ) -> Self {
        Self { auth, title: title.into(), budgets: Mutex::default(), secure, trust, mail, oidc }
    }

    /// `GET oidc/start` and `oidc/callback` in a build without `oidc`: none.
    #[cfg(not(feature = "oidc"))]
    pub(crate) fn oidc(
        &self,
        _: &str,
        _: &std::collections::HashMap<String, String>,
        _: &axum::http::HeaderMap,
    ) -> super::oidc::Page {
        match self.oidc.as_deref() {
            None => (404, vec![], "no identity provider".into()),
            Some(p) => match *p {},
        }
    }

    /// `GET oidc/start` and `oidc/callback`: pages (redirects), not JSON.
    /// Blocking (the provider is asked): run it off the async threads.
    #[cfg(feature = "oidc")]
    pub(crate) fn oidc(
        &self,
        op: &str,
        q: &std::collections::HashMap<String, String>,
        headers: &axum::http::HeaderMap,
    ) -> super::oidc::Page {
        let Some(p) = &self.oidc else { return (404, vec![], "no identity provider".into()) };
        let secure = self.secure || crate::gate::proxied_https(headers, self.trust.as_deref());
        let host = headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("localhost");
        let scheme = if secure { "https" } else { "http" };
        let redirect = p.cfg_redirect().unwrap_or_else(|| format!("{scheme}://{host}/__aio/auth/oidc/callback"));
        match op {
            "oidc/start" => super::oidc::start(p, &self.auth, q.get("redirect").map(String::as_str), &redirect, secure),
            _ => {
                let cookies = crate::http::parse_cookies(headers);
                let binder = cookies.get(super::oidc::BINDER).map(String::as_str);
                match super::oidc::callback(p, &self.auth, q, binder, &redirect, secure) {
                    Err(page) => page,
                    Ok(l) => match self.auth.issue(&l.user.id, &l.user.role) {
                        Err(e) => (503, vec![], e.to_string()),
                        Ok(token) => {
                            let session = self.cookie("login", 200, &json!({ "token": token }), headers);
                            let mut h = vec![("location".to_string(), l.back), ("set-cookie".into(), l.clear)];
                            h.extend(session.map(|c| ("set-cookie".to_string(), c)));
                            (302, h, String::new())
                        }
                    },
                }
            }
        }
    }

    /// Send `m` by the app's hook: `wait`, its outcome; else off the answer's
    /// path (true). A failure is logged.
    fn send(&self, m: super::Mail, wait: bool) -> bool {
        let Some(f) = self.mail.clone() else { return false };
        let run = move || match f(&m) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("[aio] auth: a mail ({}) could not be sent: {e}", m.subject);
                false
            }
        };
        if wait {
            return run();
        }
        std::thread::spawn(run);
        true
    }

    /// The verification mail for `id` at `to`, with a fresh token.
    fn verify_mail(&self, id: &str, to: &str) -> Result<super::Mail, Refusal> {
        let token = self.auth.one_shot("verify", id, super::VERIFY_MS)?;
        Ok(super::Mail {
            to: to.into(),
            subject: format!("Verify your {} account", self.title),
            text: format!("Your verification token: {token}\n\nPOST it to /__aio/auth/verify to activate the account."),
        })
    }

    /// Charge every key's mail budget; false when any is spent.
    fn mails(&self, keys: &[String], now: i64) -> bool {
        let mut b = self.budgets();
        keys.iter().filter(|k| !b.mails.charge(k, budget::MAILS, now)).count() == 0
    }

    /// The `Set-Cookie` for `v`'s session (a sign-in), or that clears it (a
    /// sign-out); `None` for other answers.
    fn cookie(&self, op: &str, status: u16, v: &Value, headers: &axum::http::HeaderMap) -> Option<String> {
        let secure = self.secure || crate::gate::proxied_https(headers, self.trust.as_deref());
        let opts = |max_age| crate::http::CookieOpts {
            path: Some("/".into()),
            max_age: Some(max_age),
            http_only: true,
            secure,
            same_site: Some(crate::http::SameSite::Strict),
            ..Default::default()
        };
        let name = cookie_name(&self.title);
        match v["token"].as_str() {
            Some(t) if status < 300 => crate::http::set_cookie(&name, t, &opts(self.auth.policy().ttl_ms / 1000)),
            _ if op == "logout" => crate::http::set_cookie(&name, "", &opts(0)),
            _ => None,
        }
    }

    fn budgets(&self) -> std::sync::MutexGuard<'_, Budgets> {
        self.budgets.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The signed-in caller, from their bearer token.
    fn caller(&self, bearer: Option<&str>) -> Result<Option<User>, Refusal> {
        let Some(s) = bearer.map(|t| self.auth.session(t)).transpose()?.flatten() else { return Ok(None) };
        Ok(self.auth.user(&s.user)?.map(|u| User { role: s.role, ..u }))
    }

    /// A password checked under the address's budgets.
    fn check(&self, key: &str, id: &str, password: &str, now: i64) -> Result<User, Answer> {
        if !self.budgets().work.charge(key, budget::WORK, now) {
            return Err(err(429, "too_many_attempts"));
        }
        match self.auth.verify(id, password) {
            Ok(u) => {
                self.budgets().work.refund(key);
                Ok(u)
            }
            Err(Refusal::InvalidCredentials) => Err(self.failed(key, now, "invalid_credentials")),
            Err(r) => Err(refused(&r)),
        }
    }

    fn failed(&self, key: &str, now: i64, code: &str) -> Answer {
        let mut b = self.budgets();
        b.fails.charge(key, budget::FAILS, now);
        eprintln!("[aio] auth: a failed sign-in from {key}");
        if b.fails.spent(key, budget::FAILS, now) { err(429, "too_many_attempts") } else { err(401, code) }
    }

    /// Answer `op` for a request from `ip` with `bearer` and `body`. Blocking
    /// (password hashes): run it off the async threads.
    pub(crate) fn answer(
        &self,
        post: bool,
        op: &str,
        ip: IpAddr,
        bearer: Option<&str>,
        body: &[u8],
        now: i64,
    ) -> Answer {
        self.answer_or(post, op, ip, bearer, body, now).unwrap_or_else(|a| a)
    }

    fn answer_or(
        &self,
        post: bool,
        op: &str,
        ip: IpAddr,
        bearer: Option<&str>,
        body: &[u8],
        now: i64,
    ) -> Result<Answer, Answer> {
        let key = budget::bucket(ip);
        let fail = |r: Refusal| refused(&r);
        if !post {
            return match op {
                "me" => {
                    let user = self.caller(bearer).map_err(fail)?.map(|u| json!({ "id": u.id, "role": u.role }));
                    let (mail, oidc) = (self.mail.is_some(), self.oidc.is_some());
                    let features =
                        json!({ "signup": self.auth.policy().signup, "totp": true, "oidc": oidc, "mail": mail });
                    Ok((200, json!({ "user": user, "features": features })))
                }
                _ => Err(err(404, "unknown_auth_route")),
            };
        }
        if body.len() > MAX_BODY {
            return Err(err(413, "body_too_large"));
        }
        let b: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let s = |k: &str| b[k].as_str().filter(|v| !v.is_empty());
        let me = || self.caller(bearer).map_err(fail)?.ok_or_else(|| err(401, "login_required"));
        let need_mail = || if self.mail.is_some() { Ok(()) } else { Err(err(501, "mail_not_configured")) };
        let verified = self.auth.policy().require_verified;
        match op {
            "signup" => {
                if !self.auth.policy().signup {
                    return Err(err(403, "signup_disabled"));
                }
                let (Some(id), Some(pw)) = (s("id"), s("password")) else {
                    return Err(err(400, "id_and_password_required"));
                };
                // Decided from the request alone: costs no budget.
                let email = s("email");
                if email.is_some_and(|e| !email_ok(e)) {
                    return Err(err(400, "invalid_email"));
                }
                if verified && email.is_none() {
                    return Err(err(400, "email_required"));
                }
                if let Some(r) = super::policy_refusal(id, pw) {
                    return Err(refused(&r));
                }
                if !self.budgets().signups.charge(&key, budget::SIGNUPS, now) {
                    return Err(err(429, "too_many_accounts"));
                }
                let u = self.auth.create(id, pw, None).map_err(fail)?;
                // The account exists now: a mail that fails is said, never a refusal.
                let mut sent = false;
                if let Some(e) = email {
                    self.auth.set_email(&u.id, e).map_err(fail)?;
                    if self.mail.is_some() {
                        sent = self.send(self.verify_mail(&u.id, e).map_err(fail)?, verified);
                    }
                }
                // Verification required: no session until the email is proven.
                if verified {
                    return Ok((201, json!({ "user": { "id": u.id, "role": u.role }, "verificationSent": sent })));
                }
                Ok(signed(self.auth.issue(&u.id, &u.role).map_err(fail)?, &u, 201))
            }
            "login" => {
                let (Some(id), Some(pw)) = (s("id"), s("password")) else {
                    return Err(err(400, "id_and_password_required"));
                };
                let u = self.check(&key, id, pw, now)?;
                if verified && !u.verified {
                    return Err(err(403, "email_unverified"));
                }
                match self.auth.signed_in(u).map_err(fail)? {
                    Login::Totp { pending } => Ok((200, json!({ "totpRequired": true, "pending": pending }))),
                    Login::Signed { token, user } => Ok(signed(token, &user, 200)),
                }
            }
            "totp" => {
                let (Some(pending), Some(code)) = (s("pending"), s("code")) else {
                    return Err(err(400, "pending_and_code_required"));
                };
                if !self.budgets().work.charge(&key, budget::WORK, now) {
                    return Err(err(429, "too_many_attempts"));
                }
                match self.auth.login_totp(pending, code) {
                    Ok((token, u)) => {
                        self.budgets().work.refund(&key);
                        Ok(signed(token, &u, 200))
                    }
                    Err(Refusal::InvalidCode) => Err(self.failed(&key, now, "invalid_code")),
                    Err(r) => Err(refused(&r)),
                }
            }
            "logout" => {
                if let Some(t) = bearer {
                    self.auth.revoke(t).map_err(fail)?;
                }
                Ok((200, json!({ "ok": true })))
            }
            "password" => {
                let u = me()?;
                let (Some(old), Some(new)) = (s("old"), s("new")) else {
                    return Err(err(400, "old_and_new_required"));
                };
                self.check(&key, &u.id, old, now)?;
                // Revokes every session, this one too: the caller gets a new one.
                if !self.auth.set_password(&u.id, new).map_err(fail)? {
                    return Err(err(401, "login_required"));
                }
                Ok(signed(self.auth.issue(&u.id, &u.role).map_err(fail)?, &u, 200))
            }
            "verify/request" => {
                need_mail()?;
                let u = me()?;
                let Some(to) = u.email.clone() else { return Err(err(400, "no_email_on_account")) };
                if u.verified {
                    return Ok((200, json!({ "ok": true, "alreadyVerified": true })));
                }
                if !self.mails(&[format!("verify:{}", u.id)], now) {
                    return Err(err(429, "too_many_attempts"));
                }
                let sent = self.send(self.verify_mail(&u.id, &to).map_err(fail)?, true);
                Ok(if sent { (200, json!({ "ok": true })) } else { err(503, "mail_failed") })
            }
            // The way back for an unverified account whose token expired: anonymous, so it tells nothing.
            "verify/resend" => {
                need_mail()?;
                if let Some(id) = s("id") {
                    let u = self.auth.user(id).map_err(fail)?;
                    let account = u.as_ref().map_or_else(|| super::normalize(id), |u| u.id.clone());
                    if self.mails(&[format!("resend:{key}"), format!("verify-resend:{account}")], now) {
                        let to = u.filter(|u| !u.verified && !u.id.starts_with(super::EXTERNAL_PREFIX));
                        match to.and_then(|u| Some((u.id, u.email?))) {
                            Some((id, to)) => _ = self.send(self.verify_mail(&id, &to).map_err(fail)?, false),
                            None => _ = self.auth.one_shot("verify", DECOY, super::VERIFY_MS).map_err(fail)?,
                        }
                    }
                }
                Ok((200, json!({ "ok": true })))
            }
            "verify" => {
                let token = s("token").ok_or_else(|| err(400, "token_required"))?;
                let id = self.auth.consume("verify", token).map_err(fail)?;
                let id = id.filter(|i| i != DECOY).ok_or_else(|| err(401, "invalid_or_expired_token"))?;
                self.auth.mark_verified(&id).map_err(fail)?;
                Ok((200, json!({ "ok": true })))
            }
            "reset/request" => {
                need_mail()?;
                if let Some(id) = s("id") {
                    let one = format!("reset:{key}|{}", super::normalize(id));
                    if self.mails(&[format!("reset:{key}"), one], now) {
                        // An external identity's password is its provider's: treated as a miss.
                        let u = self.auth.user(id).map_err(fail)?.filter(|u| !u.id.starts_with(super::EXTERNAL_PREFIX));
                        let subject = u.as_ref().map_or(DECOY, |u| u.id.as_str());
                        let token = self.auth.one_shot("reset", subject, super::RESET_MS).map_err(fail)?;
                        if let Some(to) = u.and_then(|u| u.email) {
                            let text = format!(
                                "Your password reset token (valid 15 minutes): {token}\n\nPOST {{ token, password }} to /__aio/auth/reset."
                            );
                            let subject = format!("Reset your {} password", self.title);
                            self.send(super::Mail { to, subject, text }, false);
                        }
                    }
                }
                Ok((200, json!({ "ok": true })))
            }
            "reset" => {
                let (Some(token), Some(pw)) = (s("token"), s("password")) else {
                    return Err(err(400, "token_and_password_required"));
                };
                // Before the token is spent: a short password is a typo to fix, not a lost link.
                if pw.chars().count() < 8 {
                    return Err(refused(&Refusal::PasswordTooShort));
                }
                let id = self.auth.consume("reset", token).map_err(fail)?;
                let id = id.filter(|i| i != DECOY).ok_or_else(|| err(401, "invalid_or_expired_token"))?;
                if id.starts_with(super::EXTERNAL_PREFIX) {
                    return Err(err(403, "external_identity"));
                }
                // An account removed since: the token is spent on nothing.
                if !self.auth.set_password(&id, pw).map_err(fail)? {
                    return Err(err(401, "invalid_or_expired_token"));
                }
                // The mailbox is proven too.
                self.auth.mark_verified(&id).map_err(fail)?;
                eprintln!("[aio] auth: a password reset completed for {id}");
                Ok((200, json!({ "ok": true })))
            }
            "totp/setup" => {
                let u = me()?;
                let secret = self.auth.totp_setup(&u.id).map_err(fail)?;
                let uri = super::totp::uri(&secret, &u.id, &self.title);
                Ok((200, json!({ "secret": secret, "uri": uri })))
            }
            // Turning the factor on or off takes the password, not only a session.
            "totp/enable" | "totp/disable" => {
                let u = me()?;
                let pw = s("password").ok_or_else(|| err(400, "password_required"))?;
                self.check(&key, &u.id, pw, now)?;
                if op == "totp/enable" {
                    self.auth.totp_enable(&u.id, s("code").unwrap_or_default()).map_err(fail)?;
                } else {
                    self.auth.totp_off(&u.id).map_err(fail)?;
                }
                Ok((200, json!({ "ok": true })))
            }
            _ => Err(err(404, "unknown_auth_route")),
        }
    }
}

/// The subject of a token minted for no account (a miss costs as a hit).
const DECOY: &str = "\u{0}decoy";

/// Looks like an address: something, `@`, a name with a dot (aio's rule).
fn email_ok(e: &str) -> bool {
    e.split_once('@').is_some_and(|(local, host)| {
        !local.is_empty() && host.rsplit_once('.').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
    })
}

/// `Authorization: Bearer <token>`.
pub(crate) fn bearer(headers: &axum::http::HeaderMap) -> Option<String> {
    let v = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    v.strip_prefix("Bearer ").map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// The session cookie of app `id` (per app: cookies ignore the port).
pub(crate) fn cookie_name(id: &str) -> String {
    format!("aio_session_{id}")
}

/// The session a request shows: its bearer, else app `id`'s session cookie.
pub(crate) fn session_token(headers: &axum::http::HeaderMap, id: &str) -> Option<String> {
    bearer(headers).or_else(|| crate::http::parse_cookies(headers).remove(&cookie_name(id)).filter(|t| !t.is_empty()))
}

/// The doors as routes.
pub(crate) fn router(doors: Arc<Doors>) -> axum::Router {
    use axum::extract::{ConnectInfo, Path};
    use axum::http::{HeaderMap, Method, StatusCode, header};
    use axum::response::IntoResponse;
    let door = move |method: Method,
                     ConnectInfo(crate::sync::Peer(from)): ConnectInfo<crate::sync::Peer>,
                     Path(op): Path<String>,
                     crate::http::Query(q): crate::http::Query,
                     headers: HeaderMap,
                     body: axum::body::Bytes| {
        let doors = doors.clone();
        async move {
            if method == Method::GET && (op == "oidc/start" || op == "oidc/callback") {
                let (d, h) = (doors.clone(), headers.clone());
                let (status, set, text) = tokio::task::spawn_blocking(move || d.oidc(&op, &q, &h))
                    .await
                    .unwrap_or_else(|_| (500, vec![], "sign-in failed".into()));
                let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let mut res = (status, [(header::CACHE_CONTROL, "no-store")], text).into_response();
                for (k, v) in set {
                    if let (Ok(k), Ok(v)) = (header::HeaderName::try_from(k), axum::http::HeaderValue::from_str(&v)) {
                        res.headers_mut().append(k, v);
                    }
                }
                return res;
            }
            let (ip, bearer) = (from.ip(), session_token(&headers, &doors.title));
            let post = method == Method::POST;
            let d = doors.clone();
            let (op, (status, v)) = tokio::task::spawn_blocking(move || {
                let a = d.answer(post, &op, ip, bearer.as_deref(), &body, crate::store::now_ms());
                (op, a)
            })
            .await
            .unwrap_or_else(|_| (String::new(), err(500, "auth_failed")));
            let cookie = doors.cookie(&op, status, &v, &headers);
            // A page that keeps its session in the cookie only gets no token to hold.
            let mut v = v;
            if headers.get(super::client::SESSION_HEADER).is_some_and(|h| h == "cookie")
                && let Some(o) = v.as_object_mut()
            {
                o.remove("token");
            }
            let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let mut res = (status, [(header::CACHE_CONTROL, "no-store")], crate::http::json(v)).into_response();
            if let Some(c) = cookie.and_then(|c| axum::http::HeaderValue::from_str(&c).ok()) {
                res.headers_mut().insert(header::SET_COOKIE, c);
            }
            res
        }
    };
    axum::Router::new()
        .route("/__aio/auth/{*op}", axum::routing::get(door.clone()).post(door))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY))
}

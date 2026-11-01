//! OpenID Connect sign-in (aio's `auth.oidc`): the authorization code flow
//! with PKCE (S256), served by the login doors at `GET oidc/start` (a 302 to
//! the provider) and `GET oidc/callback` (code → ID token, verified, then a
//! session and a 302 back to the page that asked).
//!
//! - **Discovery** from `<issuer>/.well-known/openid-configuration`, whose
//!   `issuer` must be the one configured; it and the JWKS are kept an hour.
//!   A token naming a key the JWKS lacks re-reads it (a rotated key), at
//!   most once per 30 s.
//! - **ID tokens**: RS256 or ES256 against the JWKS; `iss`, `aud`, `azp`,
//!   `exp`, `nbf`/`iat` (60 s skew), `sub` and the login's `nonce` checked.
//! - **State**: a one-shot token (10 min) carrying the PKCE verifier, the
//!   nonce, the return path and the hash of a binder cookie set at the
//!   start, so a callback replayed in another browser is refused.
//! - **Accounts**: `oidc:<issuer host/path>:<sub>`, a namespace of its own
//!   (a `sub` is case-sensitive; no local account is adopted), never
//!   password-verifiable. An email counts only with `email_verified: true`.
//!   A new identity gets an account only if sign-up is open
//!   ([`Oidc::signup`], else the app's), with [`Oidc::role`]'s role.
use base64::Engine;
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// A new account's role from its verified claims; an `Err` refuses the login.
type Role = Arc<dyn Fn(&Value) -> Result<String, String> + Send + Sync>;

/// One identity provider ([`crate::http::Http::oidc`]).
#[derive(Clone)]
pub struct Oidc {
    pub issuer: String,
    pub client_id: String,
    /// A confidential client's secret; none for a public (PKCE-only) one.
    pub client_secret: Option<String>,
    role: Option<Role>,
    signup: Option<bool>,
    redirect_uri: Option<String>,
}

impl Oidc {
    pub fn new(issuer: impl Into<String>, client_id: impl Into<String>) -> Self {
        let issuer = issuer.into().trim_end_matches('/').to_string();
        Self { issuer, client_id: client_id.into(), client_secret: None, role: None, signup: None, redirect_uri: None }
    }

    pub fn secret(mut self, s: impl Into<String>) -> Self {
        self.client_secret = Some(s.into());
        self
    }

    /// A new account's role from its claims (default `"user"`); `claims`
    /// holds `email` only when the provider verified it. Existing accounts
    /// keep theirs.
    pub fn role(mut self, f: impl Fn(&Value) -> Result<String, String> + Send + Sync + 'static) -> Self {
        self.role = Some(Arc::new(f));
        self
    }

    /// May an identity with no account get one at its first sign-in?
    /// Default: the app's own sign-up switch.
    pub fn signup(mut self, open: bool) -> Self {
        self.signup = Some(open);
        self
    }

    /// The `redirect_uri` as registered with the provider, verbatim
    /// (default: this server's `/__aio/auth/oidc/callback` as the request
    /// reached it, https behind TLS or a trusted proxy that says so).
    pub fn redirect_uri(mut self, uri: impl Into<String>) -> Self {
        self.redirect_uri = Some(uri.into());
        self
    }
}

/// The account id of `sub` at `issuer`.
pub fn account_id(issuer: &str, sub: &str) -> String {
    let iss = issuer.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
    format!("{}{iss}:{sub}", super::EXTERNAL_PREFIX)
}

/// Where a sign-in may return: a path on this site, else `/` (no open
/// redirect: no scheme, no `//host`, no `\`, no control character); bytes
/// outside printable ASCII percent-encoded.
pub fn return_path(p: Option<&str>) -> String {
    let Some(p) = p.filter(|p| p.starts_with('/') && !p[1..].starts_with(['/', '\\'])) else { return "/".into() };
    if p.chars().any(char::is_control) {
        return "/".into();
    }
    p.bytes()
        .map(|b| match b {
            b'\\' => "/".into(),
            0x21..=0x7e => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The key a token's `kid` names; a token naming none may use a sole key.
pub fn select<'a>(keys: &'a [Value], kid: Option<&str>) -> Option<&'a Value> {
    match kid {
        Some(kid) => keys.iter().find(|k| k["kid"] == kid),
        None => (keys.len() == 1).then(|| &keys[0]),
    }
}

fn b64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).ok()
}

fn b64_enc(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// The header and the signed part of a JWT, and its signature.
fn parts(token: &str) -> Option<(Value, &str, Vec<u8>)> {
    let (signed, sig) = token.rsplit_once('.')?;
    let (h, _) = signed.split_once('.')?;
    Some((serde_json::from_slice(&b64(h)?).ok()?, signed, b64(sig)?))
}

/// Whether `jwk` signed `msg` with `sig` under `alg` (RS256 or ES256).
fn signed_by(jwk: &Value, alg: &str, msg: &[u8], sig: &[u8]) -> bool {
    use ring::signature as s;
    let f = |k: &str| jwk[k].as_str().and_then(b64);
    match (alg, jwk["kty"].as_str()) {
        ("RS256", Some("RSA")) => f("n").zip(f("e")).is_some_and(|(n, e)| {
            s::RsaPublicKeyComponents { n, e }.verify(&s::RSA_PKCS1_2048_8192_SHA256, msg, sig).is_ok()
        }),
        ("ES256", Some("EC")) if jwk["crv"] == "P-256" => f("x").zip(f("y")).is_some_and(|(x, y)| {
            let point = [&[4u8][..], &x, &y].concat();
            s::UnparsedPublicKey::new(&s::ECDSA_P256_SHA256_FIXED, point).verify(msg, sig).is_ok()
        }),
        _ => false,
    }
}

/// An ID token's claims once it is proven (pure: the keys and the time are
/// arguments); else the refusal's code.
pub fn verify(token: &str, issuer: &str, client_id: &str, keys: &[Value], now_ms: i64) -> Result<Value, &'static str> {
    let (header, signed, sig) = parts(token).ok_or("oidc_bad_token")?;
    let alg = header["alg"].as_str().filter(|a| ["RS256", "ES256"].contains(a)).ok_or("oidc_alg_unsupported")?;
    let key = select(keys, header["kid"].as_str()).ok_or("oidc_unknown_kid")?;
    if !signed_by(key, alg, signed.as_bytes(), &sig) {
        return Err("oidc_bad_signature");
    }
    let body = signed.split_once('.').and_then(|(_, p)| b64(p)).ok_or("oidc_bad_token")?;
    let c: Value = serde_json::from_slice(&body).map_err(|_| "oidc_bad_token")?;
    if c["iss"].as_str().map(|i| i.trim_end_matches('/')) != Some(issuer.trim_end_matches('/')) {
        return Err("oidc_bad_iss");
    }
    let aud = &c["aud"];
    let many = aud.as_array().is_some_and(|a| a.len() > 1);
    if !(aud == client_id || aud.as_array().is_some_and(|a| a.iter().any(|x| x == client_id))) {
        return Err("oidc_bad_aud");
    }
    match &c["azp"] {
        Value::Null if many => return Err("oidc_azp_required"),
        Value::Null => {}
        azp if azp != client_id => return Err("oidc_bad_azp"),
        _ => {}
    }
    let at = |k: &str| c[k].as_f64().map(|s| (s * 1000.0) as i64);
    if at("exp").is_none_or(|exp| exp < now_ms) {
        return Err("oidc_expired");
    }
    if ["nbf", "iat"].iter().any(|k| at(k).is_some_and(|t| t > now_ms + 60_000)) {
        return Err("oidc_not_yet_valid");
    }
    if c["sub"].as_str().is_none_or(str::is_empty) {
        return Err("oidc_no_sub");
    }
    Ok(c)
}

/// What discovery says.
#[derive(Clone, Debug)]
struct Found {
    authorize: String,
    token: String,
    jwks: String,
}

const KEEP_MS: i64 = 3_600_000;
const REREAD_MS: i64 = 30_000;

/// A provider in use: its config, and what was read from it (cached).
pub(crate) struct Provider {
    pub(crate) cfg: Oidc,
    found: Mutex<Option<(Found, i64)>>,
    keys: Mutex<Option<(Vec<Value>, i64)>>,
    reread: Mutex<i64>,
}

fn get(url: &str) -> Result<Value, String> {
    let agent = crate::capabilities::agent(
        "sign-in",
        ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(10)))
            .http_status_as_error(false)
            .build(),
    );
    let mut r = agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
    let status = r.status().as_u16();
    let text = r.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| format!("{url}: {e}"))?;
    match status {
        200..300 => serde_json::from_str(&text).map_err(|e| format!("{url}: {e}")),
        s => Err(format!("{url} answered {s}")),
    }
}

/// Within `ms` of `then` (a clock stepped back is not within).
fn within(then: i64, now: i64, ms: i64) -> bool {
    (0..ms).contains(&(now - then))
}

impl Provider {
    pub(crate) fn new(cfg: Oidc) -> Self {
        Self { cfg, found: Mutex::default(), keys: Mutex::default(), reread: Mutex::default() }
    }

    /// The `redirect_uri` the app registered, if it set one.
    pub(crate) fn cfg_redirect(&self) -> Option<String> {
        self.cfg.redirect_uri.clone()
    }

    fn found(&self, now: i64) -> Result<Found, String> {
        if let Some((f, at)) = self.found.lock().unwrap_or_else(|e| e.into_inner()).clone()
            && within(at, now, KEEP_MS)
        {
            return Ok(f);
        }
        let url = format!("{}/.well-known/openid-configuration", self.cfg.issuer);
        let d = get(&url)?;
        // OIDC Discovery 4.3: a document for another issuer is not used.
        match d["issuer"].as_str().map(|i| i.trim_end_matches('/')) {
            Some(i) if i != self.cfg.issuer => {
                return Err(format!("{url} describes issuer {i:?}, not {:?}: set the issuer it uses", self.cfg.issuer));
            }
            None => eprintln!("[aio] auth: {url} names no issuer; the ID token's iss is still checked"),
            _ => {}
        }
        let s = |k: &str| d[k].as_str().map(String::from).ok_or_else(|| format!("{url}: no {k}"));
        let f = Found { authorize: s("authorization_endpoint")?, token: s("token_endpoint")?, jwks: s("jwks_uri")? };
        *self.found.lock().unwrap_or_else(|e| e.into_inner()) = Some((f.clone(), now));
        Ok(f)
    }

    /// The JWKS: cached, or `fresh` (at most once per [`REREAD_MS`]).
    fn keys(&self, jwks: &str, fresh: bool, now: i64) -> Result<Vec<Value>, String> {
        let kept = self.keys.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some((k, at)) = &kept
            && within(*at, now, KEEP_MS)
        {
            let mut last = self.reread.lock().unwrap_or_else(|e| e.into_inner());
            if !fresh || within(*last, now, REREAD_MS) {
                return Ok(k.clone());
            }
            *last = now;
        }
        let keys = get(jwks)?["keys"].as_array().cloned().unwrap_or_default();
        *self.keys.lock().unwrap_or_else(|e| e.into_inner()) = Some((keys.clone(), now));
        Ok(keys)
    }

    fn id_token(&self, token: &str, jwks: &str, now: i64) -> Result<Value, String> {
        let check = |keys: &[Value]| verify(token, &self.cfg.issuer, &self.cfg.client_id, keys, now);
        match check(&self.keys(jwks, false, now)?) {
            Err("oidc_unknown_kid") => check(&self.keys(jwks, true, now)?).map_err(String::from),
            r => r.map_err(String::from),
        }
    }

    /// The provider's sign-in page for a login carrying `state` (PKCE and
    /// nonce as `start` made them).
    fn authorize_url(
        &self,
        now: i64,
        redirect: &str,
        state: &str,
        nonce: &str,
        challenge: &str,
    ) -> Result<String, String> {
        let f = self.found(now)?;
        let q = |v: &str| percent_encoding::utf8_percent_encode(v, percent_encoding::NON_ALPHANUMERIC).to_string();
        let sep = if f.authorize.contains('?') { '&' } else { '?' };
        Ok(format!(
            "{}{sep}response_type=code&client_id={}&redirect_uri={}&scope=openid%20profile%20email&state={}&nonce={}&code_challenge={}&code_challenge_method=S256",
            f.authorize,
            q(&self.cfg.client_id),
            q(redirect),
            q(state),
            q(nonce),
            q(challenge)
        ))
    }

    /// Trade `code` for the ID token's verified claims.
    fn exchange(&self, now: i64, code: &str, redirect: &str, verifier: &str) -> Result<Value, String> {
        let f = self.found(now)?;
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect),
            ("client_id", &self.cfg.client_id),
            ("code_verifier", verifier),
        ];
        if let Some(s) = &self.cfg.client_secret {
            form.push(("client_secret", s));
        }
        let agent = crate::capabilities::agent(
            "sign-in",
            ureq::Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_secs(10)))
                .http_status_as_error(false)
                .build(),
        );
        let mut r = agent.post(&f.token).send_form(form).map_err(|e| format!("token exchange: {e}"))?;
        let status = r.status().as_u16();
        let text = r.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| e.to_string())?;
        if !(200..300).contains(&status) {
            return Err(format!("token exchange answered {status}"));
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("token exchange: {e}"))?;
        let token = v["id_token"].as_str().ok_or("token exchange: no id_token")?;
        self.id_token(token, &f.jwks, now)
    }
}

/// The cookie that binds a login to the browser that started it.
pub(crate) const BINDER: &str = "aio_oidc";

/// A random URL-safe string of `N` bytes.
fn nonce<const N: usize>() -> String {
    b64_enc(&super::random::<N>())
}

fn sha256(s: &str) -> Vec<u8> {
    ring::digest::digest(&ring::digest::SHA256, s.as_bytes()).as_ref().to_vec()
}

/// What a door answers: status, headers, a text body.
pub(crate) type Page = (u16, Vec<(String, String)>, String);

fn page(status: u16, body: &str) -> Page {
    (status, vec![], body.into())
}

/// `GET oidc/start`: the 302 to the provider, and the binder cookie.
pub(crate) fn start(p: &Provider, auth: &super::Auth, back: Option<&str>, redirect: &str, secure: bool) -> Page {
    let now = crate::store::now_ms();
    let (verifier, binder, n) = (nonce::<32>(), nonce::<32>(), nonce::<16>());
    let carried = serde_json::json!({
        "v": verifier,
        "r": return_path(back),
        "b": super::hex(&sha256(&binder)),
        "n": n,
    });
    let state = match auth.one_shot_with("oidc", "state", 10 * 60_000, Some(&carried.to_string())) {
        Ok(s) => s,
        Err(e) => return page(503, &e.to_string()),
    };
    match p.authorize_url(now, redirect, &state, &n, &b64_enc(&sha256(&verifier))) {
        Ok(to) => {
            let cookie = binder_cookie(&binder, 600, secure);
            (302, vec![("location".into(), to), ("set-cookie".into(), cookie)], String::new())
        }
        Err(e) => {
            eprintln!("[aio] auth: oidc: {e}");
            page(502, "the identity provider can't be reached")
        }
    }
}

fn binder_cookie(v: &str, max_age: i64, secure: bool) -> String {
    let o = crate::http::CookieOpts {
        path: Some("/__aio/auth/oidc".into()),
        max_age: Some(max_age),
        http_only: true,
        secure,
        same_site: Some(crate::http::SameSite::Lax),
        ..Default::default()
    };
    crate::http::set_cookie(BINDER, v, &o).unwrap_or_default()
}

/// What a callback proved: the account to sign in, and where to go.
pub(crate) struct Landed {
    pub user: super::User,
    pub back: String,
    pub clear: String,
}

/// `GET oidc/callback`: the account the provider vouched for, or the page
/// that says why not.
pub(crate) fn callback(
    p: &Provider,
    auth: &super::Auth,
    q: &std::collections::HashMap<String, String>,
    binder: Option<&str>,
    redirect: &str,
    secure: bool,
) -> Result<Landed, Page> {
    if let Some(e) = q.get("error") {
        return Err(page(400, &format!("the identity provider said: {}", e.chars().take(100).collect::<String>())));
    }
    let (Some(code), Some(state)) = (q.get("code"), q.get("state")) else {
        return Err(page(400, "missing code/state"));
    };
    let stored = auth.consume_with("oidc", state).map_err(|e| page(503, &e.to_string()))?;
    let Some(c) = stored.and_then(|(_, c)| serde_json::from_str::<Value>(&c?).ok()) else {
        return Err(page(400, "invalid or expired state"));
    };
    let s = |k: &str| c[k].as_str().unwrap_or_default().to_string();
    if binder.is_none_or(|b| super::hex(&sha256(b)) != s("b")) {
        return Err(page(400, "state not bound to this browser"));
    }
    let bad = |why: String| {
        eprintln!("[aio] auth: oidc: an ID token was refused: {why}");
        page(401, "invalid id_token")
    };
    let claims = match p.exchange(crate::store::now_ms(), code, redirect, &s("v")) {
        Ok(c) => c,
        Err(e) if e.starts_with("oidc_") => return Err(bad(e)),
        Err(e) => {
            eprintln!("[aio] auth: oidc: {e}");
            return Err(page(502, "token exchange failed"));
        }
    };
    if claims["nonce"] != s("n") {
        return Err(bad("its nonce is not this login's".into()));
    }
    let sub = claims["sub"].as_str().unwrap_or_default();
    let id = account_id(&p.cfg.issuer, sub);
    if id.chars().count() > 256 {
        return Err(bad("its subject is too long for an account id".into()));
    }
    // An email counts only when the provider verified it.
    let email = claims["email"].as_str().filter(|_| claims["email_verified"] == true);
    let store = |e: Refusal| page(503, &e.to_string());
    use super::Refusal;
    let user = match auth.user(&id).map_err(store)? {
        Some(u) => {
            if let Some(e) = email.filter(|e| u.email.as_deref() != Some(e) || !u.verified) {
                auth.set_email(&id, e).map_err(store)?;
                auth.mark_verified(&id).map_err(store)?;
            }
            auth.user(&id).map_err(store)?.unwrap_or(u)
        }
        None => {
            if !p.cfg.signup.unwrap_or(auth.policy().signup) {
                eprintln!(
                    "[aio] auth: oidc: no account {id:?} and sign-up is off: `ax auth <app> create {id:?}` admits it"
                );
                return Err(page(403, "signup_disabled"));
            }
            let mut shown = claims.clone();
            if email.is_none() {
                shown.as_object_mut().map(|o| o.remove("email"));
            }
            let role = match &p.cfg.role {
                Some(f) => f(&shown).map_err(|e| bad(format!("the role rule refused it: {e}")))?,
                None => "user".into(),
            };
            auth.create_external(&id, &role, email).map_err(store)?
        }
    };
    if auth.policy().require_verified && !user.verified {
        eprintln!("[aio] auth: oidc: {id:?} has no verified email: `ax auth <app> verify {id:?}` admits it");
        return Err(page(403, "email_unverified"));
    }
    Ok(Landed { user, back: return_path(Some(&s("r"))), clear: binder_cookie("", 0, secure) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_return_path_never_leaves_the_site() {
        assert_eq!(return_path(Some("/orders/7?x=1")), "/orders/7?x=1");
        for bad in [None, Some(""), Some("https://evil.example/"), Some("//evil.example"), Some("/\\evil"), Some("x")] {
            assert_eq!(return_path(bad), "/", "{bad:?}");
        }
        assert_eq!(return_path(Some("/a\r\nSet-Cookie: x")), "/");
        assert_eq!(return_path(Some("/café a")), "/caf%C3%A9%20a");
        assert_eq!(return_path(Some("/a\\b")), "/a/b");
    }

    #[test]
    fn a_declared_kid_must_match_and_only_a_sole_key_stands_in() {
        let keys = [serde_json::json!({ "kid": "a" }), serde_json::json!({ "kid": "b" })];
        assert_eq!(select(&keys, Some("b")).unwrap()["kid"], "b");
        assert!(select(&keys, Some("c")).is_none());
        assert!(select(&keys, None).is_none(), "two keys: a token must say which");
        assert_eq!(select(&keys[..1], None).unwrap()["kid"], "a");
        assert!(select(&keys[..1], Some("c")).is_none(), "a named kid never falls back");
    }

    #[test]
    fn external_ids_are_issuer_and_subject() {
        assert_eq!(account_id("https://login.example.com/", "AbC"), "oidc:login.example.com:AbC");
        assert_eq!(account_id("http://127.0.0.1:9/realm", "x"), "oidc:127.0.0.1:9/realm:x");
    }
}

//! OpenID Connect sign-in against a provider played on loopback: discovery,
//! the code + PKCE flow, the ID token's proofs, accounts and the JWKS cache.
use aio::auth::oidc::{Oidc, verify};
use aio::auth::{Auth, Policy};
use aio::prelude::*;
use base64::Engine;
use ring::signature::{self, KeyPair};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[cell(persist, shared)]
pub struct Desk {
    pub n: u64,
}

#[methods]
impl Desk {}

/// Test-only key, made for these tests (`openssl genpkey -algorithm RSA`, as PKCS#1 DER).
const RSA: &[u8] = include_bytes!("fixtures/oidc-test-rsa.der");

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn json_answer(v: Value) -> axum::response::Response {
    use axum::response::IntoResponse;
    ([(axum::http::header::CONTENT_TYPE, "application/json")], v.to_string()).into_response()
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

/// A signing key and its JWK.
enum Key {
    Rsa(signature::RsaKeyPair),
    Ec(signature::EcdsaKeyPair),
}

impl Key {
    fn rsa() -> Self {
        Key::Rsa(signature::RsaKeyPair::from_der(RSA).unwrap())
    }

    fn ec() -> Self {
        let rng = ring::rand::SystemRandom::new();
        let alg = &signature::ECDSA_P256_SHA256_FIXED_SIGNING;
        let pk8 = signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
        Key::Ec(signature::EcdsaKeyPair::from_pkcs8(alg, pk8.as_ref(), &rng).unwrap())
    }

    fn jwk(&self, kid: &str) -> Value {
        match self {
            Key::Rsa(k) => {
                let c = ring::rsa::PublicKeyComponents::<Vec<u8>>::from(k.public());
                json!({ "kty": "RSA", "kid": kid, "n": b64(&c.n), "e": b64(&c.e) })
            }
            Key::Ec(k) => {
                let p = k.public_key().as_ref();
                json!({ "kty": "EC", "crv": "P-256", "kid": kid, "x": b64(&p[1..33]), "y": b64(&p[33..]) })
            }
        }
    }

    fn sign(&self, kid: Option<&str>, claims: &Value) -> String {
        let alg = match self {
            Key::Rsa(_) => "RS256",
            Key::Ec(_) => "ES256",
        };
        let h = json!({ "alg": alg, "kid": kid, "typ": "JWT" });
        let h = if kid.is_none() { json!({ "alg": alg }) } else { h };
        let msg = format!("{}.{}", b64(h.to_string().as_bytes()), b64(claims.to_string().as_bytes()));
        let rng = ring::rand::SystemRandom::new();
        let sig = match self {
            Key::Rsa(k) => {
                let mut sig = vec![0; k.public().modulus_len()];
                k.sign(&signature::RSA_PKCS1_SHA256, &rng, msg.as_bytes(), &mut sig).unwrap();
                sig
            }
            Key::Ec(k) => k.sign(&rng, msg.as_bytes()).unwrap().as_ref().to_vec(),
        };
        format!("{msg}.{}", b64(&sig))
    }
}

const CLIENT: &str = "app-client";

fn claims(iss: &str, sub: &str) -> Value {
    json!({ "iss": iss, "aud": CLIENT, "sub": sub, "exp": now() + 300, "iat": now() })
}

#[test]
fn an_id_token_is_proven_by_its_signature_and_every_claim() {
    let iss = "https://idp.example";
    let (rsa, ec) = (Key::rsa(), Key::ec());
    let keys = [rsa.jwk("r1"), ec.jwk("e1")];
    // One moment for the whole test: claims dated by a later clock read would
    // drift past `ms` (an exp of now - 1 is still ahead of a start a second ago).
    let at = now();
    let ms = at * 1000;
    let ok = |c: &Value, k: &Key, kid| verify(&k.sign(kid, c), iss, CLIENT, &keys, ms);
    let c = claims(iss, "alice");
    assert_eq!(ok(&c, &rsa, Some("r1")).unwrap()["sub"], "alice", "RS256");
    assert_eq!(ok(&c, &ec, Some("e1")).unwrap()["sub"], "alice", "ES256");
    // Signed by one key, claiming another; no kid among two keys; an unknown kid.
    assert_eq!(ok(&c, &ec, Some("r1")), Err("oidc_bad_signature"));
    assert_eq!(ok(&c, &rsa, None), Err("oidc_unknown_kid"));
    assert_eq!(ok(&c, &rsa, Some("r9")), Err("oidc_unknown_kid"));
    let one = [rsa.jwk("r1")];
    assert!(verify(&rsa.sign(None, &c), iss, CLIENT, &one, ms).is_ok(), "a sole key stands in for no kid");
    // A changed payload breaks the signature.
    let t = rsa.sign(Some("r1"), &c);
    let mut p: Vec<&str> = t.split('.').collect();
    let forged = b64(claims(iss, "mallory").to_string().as_bytes());
    p[1] = &forged;
    assert_eq!(verify(&p.join("."), iss, CLIENT, &keys, ms), Err("oidc_bad_signature"));
    assert_eq!(verify("not.a-token", iss, CLIENT, &keys, ms), Err("oidc_bad_token"));
    let none = format!("{}.{}.", b64(br#"{"alg":"none"}"#), b64(c.to_string().as_bytes()));
    assert_eq!(verify(&none, iss, CLIENT, &keys, ms), Err("oidc_alg_unsupported"));
    // Each claim.
    let with = |k: &str, v: Value| {
        let mut c = claims(iss, "alice");
        c[k] = v;
        ok(&c, &rsa, Some("r1"))
    };
    assert_eq!(with("iss", json!("https://other.example")), Err("oidc_bad_iss"));
    assert!(with("iss", json!("https://idp.example/")).is_ok(), "a trailing slash is the same issuer");
    assert_eq!(with("aud", json!("another-app")), Err("oidc_bad_aud"));
    assert!(with("aud", json!([CLIENT])).is_ok());
    assert_eq!(with("aud", json!([CLIENT, "x"])), Err("oidc_azp_required"), "many audiences: azp names the client");
    assert_eq!(with("azp", json!("x")), Err("oidc_bad_azp"));
    assert_eq!(with("exp", json!(at - 1)), Err("oidc_expired"));
    assert_eq!(with("exp", Value::Null), Err("oidc_expired"));
    assert_eq!(with("nbf", json!(at + 600)), Err("oidc_not_yet_valid"));
    assert!(with("nbf", json!(at + 30)).is_ok(), "within the skew");
    assert_eq!(with("iat", json!(at + 600)), Err("oidc_not_yet_valid"));
    assert_eq!(with("sub", json!("")), Err("oidc_no_sub"));
}

/// The provider: its keys, the codes it issued, how often its JWKS was read.
#[derive(Default)]
struct Idp {
    keys: Vec<Value>,
    codes: HashMap<String, (Value, String)>,
    jwks_reads: usize,
    /// The discovery document's `issuer`, if not the real one.
    claims_issuer: Option<String>,
}

type Shared = Arc<Mutex<Idp>>;

async fn idp(state: Shared) -> String {
    use axum::routing::{get, post};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let iss = format!("http://{}", l.local_addr().unwrap());
    let (s1, s2, s3, i) = (state.clone(), state.clone(), state, iss.clone());
    let app = axum::Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || {
                let said = s1.lock().unwrap().claims_issuer.clone().unwrap_or(i.clone());
                let i = i.clone();
                async move {
                    json_answer(json!({ "issuer": said, "authorization_endpoint": format!("{i}/authorize"), "token_endpoint": format!("{i}/token"), "jwks_uri": format!("{i}/jwks") }))
                }
            }),
        )
        .route(
            "/jwks",
            get(move || {
                let mut s = s2.lock().unwrap();
                s.jwks_reads += 1;
                let keys = s.keys.clone();
                async move { json_answer(json!({ "keys": keys })) }
            }),
        )
        .route(
            "/token",
            post(move |body: String| {
                let f = pairs(&body.replace('+', " "));
                let got = s3.lock().unwrap().codes.remove(&f["code"]);
                async move {
                    let Some((token, challenge)) = got else { return (axum::http::StatusCode::BAD_REQUEST, "bad code").into_response() };
                    let proof = b64(ring::digest::digest(&ring::digest::SHA256, f["code_verifier"].as_bytes()).as_ref());
                    if proof != challenge || f["client_id"] != CLIENT || f["grant_type"] != "authorization_code" {
                        return (axum::http::StatusCode::BAD_REQUEST, "bad PKCE").into_response();
                    }
                    json_answer(json!({ "id_token": token["t"], "token_type": "Bearer" })).into_response()
                }
            }),
        );
    tokio::spawn(async move { axum::serve(l, app).await });
    iss
}

use axum::response::IntoResponse;

/// One request to the app: status, lowercased head, body.
async fn raw(addr: &str, path: &str, cookie: Option<&str>) -> (u16, String, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let c = cookie.map(|c| format!("Cookie: {c}\r\n")).unwrap_or_default();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n{c}Connection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    (head[9..12].parse().unwrap(), head.to_string(), body.to_string())
}

fn header<'a>(head: &'a str, name: &str) -> Vec<&'a str> {
    head.lines()
        .filter_map(|l| l.split_once(": ").filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v))
        .collect()
}

fn query(url: &str) -> HashMap<String, String> {
    pairs(url.split_once('?').unwrap().1)
}

/// `a=1&b=2`, decoded.
fn pairs(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.into(), percent_encoding::percent_decode_str(v).decode_utf8_lossy().into_owned()))
        .collect()
}

struct World {
    app: String,
    iss: String,
    idp: Shared,
    auth: Arc<Auth>,
    key: Key,
    kid: &'static str,
}

async fn world(policy: Policy, oidc: impl FnOnce(Oidc) -> Oidc) -> World {
    let idp_state: Shared = Default::default();
    let key = Key::rsa();
    idp_state.lock().unwrap().keys = vec![key.jwk("k1")];
    let iss = idp(idp_state.clone()).await;
    let auth = Arc::new(Auth::memory(policy).unwrap());
    let http = aio::http::Http::default().oidc(oidc(Oidc::new(&iss, CLIENT)));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let app = l.local_addr().unwrap().to_string();
    let opts = aio::sync::Options { token: "secret".into(), auth: Some(auth.clone()), http, ..Default::default() };
    tokio::spawn(aio::sync::serve_with(Handle::<Desk>::memory(), l, opts));
    World { app, iss, idp: idp_state, auth, key, kid: "k1" }
}

impl World {
    /// Sign in as `extra` says (on top of the usual claims for `sub`); the
    /// callback's status, head and body.
    async fn login(&self, sub: &str, back: &str, extra: Value) -> (u16, String, String) {
        self.login_at(&format!("/__aio/auth/oidc/start?redirect={back}"), sub, extra).await
    }

    /// [`Self::login`] begun at `start`.
    async fn login_at(&self, start: &str, sub: &str, extra: Value) -> (u16, String, String) {
        let (mut s, mut head, body) = raw(&self.app, start, None).await;
        let mut binder = header(&head, "set-cookie")[0].split(';').next().unwrap().to_string();
        // An app's sign-in: the person confirms it in the browser first.
        if start.contains("port=") {
            assert_eq!(s, 200, "{head}");
            let go = body.split("name=\"go\" value=\"").nth(1).unwrap().split('"').next().unwrap();
            (s, head, _) = raw(&self.app, &format!("{start}&go={go}"), Some(&binder)).await;
            binder = header(&head, "set-cookie")[0].split(';').next().unwrap().to_string();
        }
        assert_eq!(s, 302, "{head}");
        let to = header(&head, "location")[0].to_string();
        let q = query(&to);
        assert_eq!((q["response_type"].as_str(), q["code_challenge_method"].as_str()), ("code", "S256"));
        assert_eq!(q["redirect_uri"], format!("http://{}/__aio/auth/oidc/callback", self.app));
        let mut c = claims(&self.iss, sub);
        c["nonce"] = json!(q["nonce"]);
        for (k, v) in extra.as_object().unwrap() {
            c[k] = v.clone();
        }
        let code = format!("code-{sub}-{}", q["state"].len());
        let t = self.key.sign(Some(self.kid), &c);
        self.idp.lock().unwrap().codes.insert(code.clone(), (json!({ "t": t }), q["code_challenge"].clone()));
        let state = percent_encoding::utf8_percent_encode(&q["state"], percent_encoding::NON_ALPHANUMERIC).to_string();
        raw(&self.app, &format!("/__aio/auth/oidc/callback?code={code}&state={state}"), Some(&binder)).await
    }
}

/// `POST` `body` to the app's door `op`: status, answer.
async fn post(addr: &str, op: &str, body: Value) -> (u16, Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = body.to_string();
    let req = format!(
        "POST /__aio/auth/{op} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    (head[9..12].parse().unwrap(), serde_json::from_str(body).unwrap_or_default())
}

fn sha_hex(s: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, s.as_bytes()).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// The session a signed-in callback set.
fn session(head: &str) -> Option<String> {
    header(head, "set-cookie")
        .into_iter()
        .find_map(|c| c.strip_prefix("aio_session_desk=").map(|v| v.split(';').next().unwrap().to_string()))
        .filter(|t| !t.is_empty())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_provider_sign_in_makes_an_account_and_returns_to_its_page() {
    let w = world(Policy { signup: true, ..Default::default() }, |o| o).await;
    let (s, head, _) = w.login("Alice", "/orders/7", json!({ "email": "a@example.com", "email_verified": true })).await;
    assert_eq!((s, header(&head, "location")), (302, vec!["/orders/7"]), "{head}");
    let token = session(&head).expect("a session");
    assert!(header(&head, "set-cookie").iter().any(|c| c.starts_with("aio_oidc=;")), "the binder is cleared");
    let id = aio::auth::oidc::account_id(&w.iss, "Alice");
    let u = w.auth.session(&token).unwrap().unwrap();
    assert_eq!((u.user.as_str(), u.role.as_str()), (id.as_str(), "user"));
    let a = w.auth.user(&id).unwrap().unwrap();
    assert_eq!((a.email.as_deref(), a.verified), (Some("a@example.com"), true));
    // Never a password login, and a sub is case-sensitive: `alice` is someone else.
    assert!(w.auth.verify(&id, "").is_err());
    let (s, head, _) = w.login("alice", "https://evil.example/", json!({})).await;
    assert_eq!((s, header(&head, "location")), (302, vec!["/"]), "no open redirect");
    assert_ne!(w.auth.session(&session(&head).unwrap()).unwrap().unwrap().user, id);
    let (_, _, me) = raw(&w.app, "/__aio/auth/me", None).await;
    assert_eq!(serde_json::from_str::<Value>(&me).unwrap()["features"]["oidc"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_callback_proves_its_state_browser_and_nonce() {
    let w = world(Policy { signup: true, ..Default::default() }, |o| o).await;
    // No state, a made-up one, a callback without the binder of the browser that started it.
    assert_eq!(raw(&w.app, "/__aio/auth/oidc/callback?code=x", None).await.0, 400);
    assert_eq!(raw(&w.app, "/__aio/auth/oidc/callback?code=x&state=aiot_nope", None).await.0, 400);
    assert_eq!(raw(&w.app, "/__aio/auth/oidc/callback?error=access_denied", None).await.0, 400);
    let (s, head, _) = raw(&w.app, "/__aio/auth/oidc/start", None).await;
    assert_eq!(s, 302);
    let state = query(header(&head, "location")[0])["state"].clone();
    let path = format!("/__aio/auth/oidc/callback?code=x&state={state}");
    let (s, _, body) = raw(&w.app, &path, Some("aio_oidc=someone-elses")).await;
    assert_eq!((s, body.as_str()), (400, "state not bound to this browser"));
    assert_eq!(raw(&w.app, &path, None).await.0, 400, "the state was spent");
    // A token minted for another login (its nonce) is refused, and no session is made.
    let (s, head, _) = w.login("bob", "/", json!({ "nonce": "another-login" })).await;
    assert_eq!((s, session(&head)), (401, None));
    let (s, _, _) = w.login("bob", "/", json!({ "aud": "another-app" })).await;
    assert_eq!(s, 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_up_and_verification_rules_hold_for_provider_accounts() {
    // Sign-up off: an unknown identity is refused; one the operator seeded is let in with its role.
    let w = world(Policy::default(), |o| o).await;
    let (s, _, body) = w.login("carol", "/", json!({})).await;
    assert_eq!((s, body.as_str()), (403, "signup_disabled"));
    assert!(w.auth.users().unwrap().is_empty());
    let id = aio::auth::oidc::account_id(&w.iss, "carol");
    w.auth.create_external(&id, "admin", None).unwrap();
    let (s, head, _) = w.login("carol", "/", json!({})).await;
    assert_eq!(s, 302);
    assert_eq!(w.auth.session(&session(&head).unwrap()).unwrap().unwrap().role, "admin");
    // The provider's own switch opens it with password sign-up closed; the role rule names the role.
    let w = world(Policy::default(), |o| {
        o.signup(true).role(|c| Ok(if c["email"] == "boss@example.com" { "admin" } else { "user" }.into()))
    })
    .await;
    let (_, head, _) = w.login("d", "/", json!({ "email": "boss@example.com", "email_verified": true })).await;
    assert_eq!(w.auth.session(&session(&head).unwrap()).unwrap().unwrap().role, "admin");
    // An unverified email counts for nothing: no role from it, not stored.
    let (_, head, _) = w.login("e", "/", json!({ "email": "boss@example.com", "email_verified": false })).await;
    assert_eq!(w.auth.session(&session(&head).unwrap()).unwrap().unwrap().role, "user");
    assert_eq!(w.auth.user(&aio::auth::oidc::account_id(&w.iss, "e")).unwrap().unwrap().email, None);
    // Verification required: only a provider-verified email signs in.
    let w = world(Policy { signup: true, require_verified: true, ..Default::default() }, |o| o).await;
    let (s, _, body) = w.login("f", "/", json!({ "email": "f@example.com" })).await;
    assert_eq!((s, body.as_str()), (403, "email_unverified"));
    let (s, _, _) = w.login("f", "/", json!({ "email": "f@example.com", "email_verified": true })).await;
    assert_eq!(s, 302, "the same account, now vouched for");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rotated_key_is_found_by_one_jwks_reread_and_a_wrong_issuer_is_refused() {
    let mut w = world(Policy { signup: true, ..Default::default() }, |o| o).await;
    assert_eq!(w.login("g", "/", json!({})).await.0, 302);
    let reads = w.idp.lock().unwrap().jwks_reads;
    // The provider rotates: its new key is not in the cached JWKS.
    let key = Key::ec();
    w.idp.lock().unwrap().keys = vec![key.jwk("k2")];
    (w.key, w.kid) = (key, "k2");
    assert_eq!(w.login("g", "/", json!({})).await.0, 302, "a re-read finds it");
    assert_eq!(w.idp.lock().unwrap().jwks_reads, reads + 1);
    // An unknown kid again within the throttle: no second re-read, refused.
    (w.key, w.kid) = (Key::ec(), "k3");
    assert_eq!(w.login("g", "/", json!({})).await.0, 401);
    assert_eq!(w.idp.lock().unwrap().jwks_reads, reads + 1, "at most one re-read per 30 s");
    // A discovery document for another issuer is not used.
    let w = world(Policy { signup: true, ..Default::default() }, |o| o).await;
    w.idp.lock().unwrap().claims_issuer = Some("https://someone.else".into());
    assert_eq!(raw(&w.app, "/__aio/auth/oidc/start", None).await.0, 502);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_gets_its_session_at_its_loopback_and_only_with_its_verifier() {
    let w = world(Policy { signup: true, ..Default::default() }, |o| o).await;
    let start = |claim: &str| format!("/__aio/auth/oidc/start?port=9&claim={claim}");
    for bad in ["/__aio/auth/oidc/start?port=9", "/__aio/auth/oidc/start?port=0&claim=x", &start("nothex")] {
        assert_eq!(raw(&w.app, bad, None).await.0, 400, "{bad}");
    }
    // A link to an app's sign-in made elsewhere only asks: no trip to the
    // provider without this browser's confirmation, its tag bound to the
    // browser's binder, the port and the claim.
    let (s, head, body) = raw(&w.app, &start(&sha_hex("v1")), None).await;
    assert!(s == 200 && body.contains("port 9") && header(&head, "location").is_empty(), "{head}");
    let binder = header(&head, "set-cookie")[0].split(';').next().unwrap().to_string();
    let go = body.split("name=\"go\" value=\"").nth(1).unwrap().split('"').next().unwrap();
    for (path, cookie) in [
        (format!("{}&go={go}", start(&sha_hex("v1"))), Some("aio_oidc=someone-elses")),
        (format!("{}&go={go}", start(&sha_hex("v1"))), None),
        (format!("{}&go={go}", start(&sha_hex("v2"))), Some(binder.as_str())),
    ] {
        assert_eq!(raw(&w.app, &path, cookie).await.0, 200, "{path} {cookie:?}");
    }
    // The answer goes to the app's loopback port, with a code and no session cookie.
    let (s, head, _) = w.login_at(&start(&sha_hex("v1")), "Hana", json!({})).await;
    let to = header(&head, "location")[0].to_string();
    assert!(s == 302 && to.starts_with("http://127.0.0.1:9/?code=aiot_"), "{head}");
    assert_eq!(session(&head), None);
    let code = query(&to)["code"].clone();
    // Someone else's verifier is refused, and spends the code.
    assert_eq!(post(&w.app, "oidc/redeem", json!({ "code": code, "verifier": "v2" })).await.0, 400);
    let (s, v) = post(&w.app, "oidc/redeem", json!({ "code": code, "verifier": "v1" })).await;
    assert_eq!((s, v["error"].as_str()), (400, Some("sso_expired")));
    // The client does it all: its URL, the browser's trip, its loopback, the redeem.
    let client = aio::auth::client::Client::new(w.app.clone(), None);
    let sso = client.sso().unwrap();
    let path = sso.url.split_once(w.app.as_str()).unwrap().1.to_string();
    let done = tokio::spawn(async move { client.sso_done(sso).await });
    let (_, head, _) = w.login_at(&path, "Hana", json!({})).await;
    let to = header(&head, "location")[0].to_string();
    let loopback = to.strip_prefix("http://").unwrap().split_once('/').unwrap().0.to_string();
    assert_eq!(raw(&loopback, "/favicon.ico", None).await.0, 400, "what else knocks is turned away");
    let (s, _, body) = raw(&loopback, to.split_once(&loopback).unwrap().1, None).await;
    assert_eq!((s, body.as_str()), (200, "Signed in. Go back to the app; this tab can be closed."));
    let signed = done.await.unwrap().unwrap();
    assert_eq!(signed.user, aio::auth::oidc::account_id(&w.iss, "Hana"));
    assert_eq!(w.auth.session(&signed.token).unwrap().unwrap().user, signed.user);
}

/// The kit's `SignIn` offers SSO when the app has a provider; a desktop
/// app's sign-in comes back to it.
#[cfg(feature = "kit")]
#[test]
fn sign_in_offers_single_sign_on_and_the_browser_brings_the_session_back() {
    use aio::kit::SignIn;
    use aio::ui::prelude::*;
    thread_local! {
        static AT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
        static WHO: std::cell::RefCell<Option<aio::auth::client::Signed>> = const { std::cell::RefCell::new(None) };
    }
    let w = aio::rt().block_on(world(Policy { signup: true, ..Default::default() }, |o| o));
    AT.set(w.app.clone());
    fn form() -> View {
        let client = aio::auth::client::Client::new(AT.with_borrow(Clone::clone), None);
        rsx! { SignIn { client, sso_label: "Use the company account", on_signed: move |s| WHO.set(Some(s)) } }
    }
    let mut ui = aio::testing::TestUi::new(form);
    for _ in 0..500 {
        if ui.present("signinSso") {
            break;
        }
        ui.wait(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        (ui.text("signinSso").as_str(), ui.attr("signinSso", "target").as_deref()),
        ("Use the company account", Some("_blank"))
    );
    let href = ui.attr("signinSso", "href").unwrap();
    let path = href.split_once(w.app.as_str()).unwrap().1.to_string();
    let to =
        aio::rt().block_on(async { header(&w.login_at(&path, "Ida", json!({})).await.1, "location")[0].to_string() });
    let loopback = to.strip_prefix("http://").unwrap().split_once('/').unwrap().0.to_string();
    assert_eq!(aio::rt().block_on(raw(&loopback, to.split_once(&loopback).unwrap().1, None)).0, 200);
    let id = aio::auth::oidc::account_id(&w.iss, "Ida");
    ui.until("signed in", || WHO.with_borrow(|w| w.as_ref().map(|s| s.user.as_str()) == Some(id.as_str())));
}

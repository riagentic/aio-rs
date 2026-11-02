use aio::auth::{Auth, Policy};
use aio::prelude::*;
use aio::sync::{Options, Remote, serve_with};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

#[cell(persist, shared)]
pub struct Board {
    pub notes: BTreeMap<Uuid, String>,
    /// Who wrote last, as the method sees its caller.
    pub last: Option<By>,
    /// And their role.
    pub role: Option<String>,
}

#[methods]
impl Board {
    #[client]
    pub fn add(&mut self, cx: &Cx, text: String) -> Result<Uuid> {
        let id = cx.id();
        self.notes.insert(id, text);
        self.last = Some(cx.by().clone());
        self.role = cx.role().map(Into::into);
        Ok(id)
    }
}

const PW: &str = "correct horse battery";

async fn server(policy: Policy) -> (Handle<Board>, Arc<Auth>, String) {
    let h = Handle::<Board>::memory();
    let auth = Arc::new(Auth::memory(policy).unwrap());
    auth.create("ann", PW, Some("admin")).unwrap();
    auth.create("bob", PW, None).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = Options { token: "secret".into(), auth: Some(auth.clone()), ..Default::default() };
    tokio::spawn(serve_with(h.clone(), l, opts));
    (h, auth, addr)
}

/// One plain HTTP/1.1 request; the status, the lower-cased head and the JSON body.
async fn http(addr: &str, method: &str, path: &str, bearer: Option<&str>, body: Value) -> (u16, String, Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = body.to_string();
    let auth = bearer.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    (head[9..12].parse().unwrap(), head.to_ascii_lowercase(), serde_json::from_str(body).unwrap_or_default())
}

async fn post(addr: &str, op: &str, bearer: Option<&str>, body: Value) -> (u16, Value) {
    let (s, _, v) = http(addr, "POST", &format!("/__aio/auth/{op}"), bearer, body).await;
    (s, v)
}

async fn login(addr: &str, id: &str) -> String {
    let (s, v) = post(addr, "login", None, json!({ "id": id, "password": PW })).await;
    assert_eq!(s, 200, "{v}");
    v["token"].as_str().unwrap().to_string()
}

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..500 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out: {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn with_accounts_the_user_is_the_sessions_not_the_clients() {
    let (h, _, addr) = server(Policy::default()).await;
    // The shared key opens nothing any more; neither does a token in the URL.
    let e = Remote::<Board>::connect(&addr, "secret", "ann").await.err().unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
    let t = login(&addr, "bob").await;
    let url = format!("ws://{addr}/ws?token={t}");
    match tokio_tungstenite::connect_async(url).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => assert_eq!(r.status(), 401),
        other => panic!("a token in the URL must be refused: {:?}", other.map(|_| ())),
    }
    // Bob can't say he is Ann.
    let e = Remote::<Board>::connect(&addr, &t, "ann").await.err().unwrap();
    assert!(e.to_string().contains("signed in as bob"), "{e}");
    let bob = Remote::<Board>::connect(&addr, &t, "bob").await.unwrap();
    bob.add("hi".into()).unwrap();
    until("confirmed", || bob.pending() == 0).await;
    assert_eq!(h.store(|s| s.events().unwrap()).last().unwrap().by, By::User("bob".into()));
    // The method saw the proven caller, on the server and in the prediction alike.
    assert_eq!(h.state().last, Some(By::User("bob".into())));
    assert_eq!(*bob.state(), *h.state());
    // And their role, read from the account at this call.
    let ann = Remote::<Board>::connect(&addr, &login(&addr, "ann").await, "ann").await.unwrap();
    ann.add("yo".into()).unwrap();
    until("ann's call is in", || h.state().last == Some(By::User("ann".into()))).await;
    assert_eq!(h.state().role.as_deref(), Some("admin"));
    until("her copy follows", || ann.pending() == 0 && *ann.state() == *h.state()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_session_closes_its_socket_and_stays_out() {
    let (_, auth, addr) = server(Policy::default()).await;
    let (t1, t2, other) = (login(&addr, "ann").await, login(&addr, "ann").await, login(&addr, "bob").await);
    let one = Remote::<Board>::connect(&addr, &t1, "ann").await.unwrap();
    let two = Remote::<Board>::connect(&addr, &t2, "ann").await.unwrap();
    let bob = Remote::<Board>::connect(&addr, &other, "bob").await.unwrap();
    // Logout ends that session's socket only.
    assert_eq!(post(&addr, "logout", Some(&t1), json!({})).await.0, 200);
    until("closed on logout", || !one.online()).await;
    assert!(two.online() && bob.online());
    // A new password ends every session of the user.
    assert!(auth.set_password("ann", "another long password").unwrap());
    until("closed on a new password", || !two.online()).await;
    assert!(bob.online());
    // The dead tokens are refused, so the copies stop trying.
    for t in [&t1, &t2] {
        let e = Remote::<Board>::connect(&addr, t, "ann").await.err().unwrap();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(!one.online() && !two.online());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_login_doors() {
    let (_, _, addr) = server(Policy::default()).await;
    let (s, head, v) = http(&addr, "GET", "/__aio/auth/me", None, Value::Null).await;
    assert_eq!((s, &v["user"]), (200, &Value::Null));
    assert!(head.contains("cache-control: no-store"));
    assert_eq!(v["features"]["signup"], false);
    // Signup is off unless the app turns it on.
    assert_eq!(post(&addr, "signup", None, json!({ "id": "eve", "password": PW })).await.0, 403);
    assert_eq!(post(&addr, "login", None, json!({ "id": "ann" })).await.0, 400);
    let t = login(&addr, " ann ").await;
    let (_, _, me) = http(&addr, "GET", "/__aio/auth/me", Some(&t), Value::Null).await;
    assert_eq!(me["user"], json!({ "id": "ann", "role": "admin" }));
    // A new password needs the old one, and hands back a new session.
    let (s, v) = post(&addr, "password", Some(&t), json!({ "old": "wrong password", "new": "x" })).await;
    assert_eq!((s, v["error"].as_str()), (401, Some("invalid_credentials")));
    let (s, v) = post(&addr, "password", Some(&t), json!({ "old": PW, "new": "a new long password" })).await;
    assert_eq!(s, 200, "{v}");
    let (_, _, me) = http(&addr, "GET", "/__aio/auth/me", Some(&t), Value::Null).await;
    assert_eq!(me["user"], Value::Null, "the old session ended");
    let fresh = v["token"].as_str().unwrap();
    let (_, _, me) = http(&addr, "GET", "/__aio/auth/me", Some(fresh), Value::Null).await;
    assert_eq!(me["user"]["id"], "ann");
}

#[tokio::test(flavor = "multi_thread")]
async fn signup_when_allowed_is_bounded_and_checked() {
    let (_, _, addr) = server(Policy { signup: true, ..Default::default() }).await;
    let (s, v) = post(&addr, "signup", None, json!({ "id": "eve", "password": PW })).await;
    assert_eq!((s, v["user"]["role"].as_str()), (201, Some("user")));
    assert!(v["token"].as_str().unwrap().starts_with("aios_"));
    assert_eq!(post(&addr, "signup", None, json!({ "id": "EVE", "password": PW })).await.0, 409);
    // Refused from the request alone: no budget spent.
    for _ in 0..20 {
        assert_eq!(post(&addr, "signup", None, json!({ "id": "x", "password": "1" })).await.0, 400);
    }
    let mut last = 0;
    for i in 0..10 {
        last = post(&addr, "signup", None, json!({ "id": format!("u{i}"), "password": PW })).await.0;
    }
    assert_eq!(last, 429, "ten accounts an hour per address");
}

#[tokio::test(flavor = "multi_thread")]
async fn guessing_is_budgeted_but_the_right_password_is_served() {
    let (_, _, addr) = server(Policy::default()).await;
    let mut answers = vec![];
    // Spread over accounts and unknown ids, so no account locks.
    for i in 0..11 {
        let id = ["ann", "bob", "nobody"][i % 3];
        answers.push(post(&addr, "login", None, json!({ "id": id, "password": "wrong password" })).await.0);
    }
    assert!(answers[..9].iter().all(|&s| s == 401), "{answers:?}");
    assert_eq!(answers[9..], [429, 429]);
    login(&addr, "bob").await;
    // Five wrong for one account lock it, said only to the right password.
    let (_, _, addr) = server(Policy::default()).await;
    for _ in 0..5 {
        post(&addr, "login", None, json!({ "id": "ann", "password": "wrong password" })).await;
    }
    let (s, v) = post(&addr, "login", None, json!({ "id": "ann", "password": PW })).await;
    assert_eq!((s, v["error"].as_str()), (423, Some("account_locked")));
}

#[tokio::test(flavor = "multi_thread")]
async fn totp_needs_the_password_to_turn_on_or_off() {
    let (_, _, addr) = server(Policy::default()).await;
    let t = login(&addr, "ann").await;
    let (s, v) = post(&addr, "totp/setup", Some(&t), json!({})).await;
    assert_eq!(s, 200);
    let secret = v["secret"].as_str().unwrap().to_string();
    assert!(v["uri"].as_str().unwrap().starts_with("otpauth://totp/"));
    let key = aio::auth::totp::unbase32(&secret).unwrap();
    // One step read once: the server takes its step ± one, and a hash takes
    // seconds, so a code read per call could fall out of it at a step's end.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    let step = now / aio::auth::totp::STEP_MS;
    let first = aio::auth::totp::code(&key, step);
    assert_eq!(post(&addr, "totp/enable", Some(&t), json!({ "code": first })).await.0, 400);
    let (s, _) = post(&addr, "totp/enable", Some(&t), json!({ "code": first, "password": "wrong password" })).await;
    assert_eq!(s, 401);
    let (s, v) = post(&addr, "totp/enable", Some(&t), json!({ "code": first, "password": PW })).await;
    assert_eq!(s, 200, "{v}");
    // Now a login takes a second step, and its code (a newer step) works once.
    let (s, v) = post(&addr, "login", None, json!({ "id": "ann", "password": PW })).await;
    assert_eq!((s, v["totpRequired"].as_bool()), (200, Some(true)));
    let c = aio::auth::totp::code(&key, step + 1);
    let (s, body) = post(&addr, "totp", None, json!({ "pending": v["pending"], "code": c })).await;
    assert_eq!(s, 200, "{body}");
    let (_, v) = post(&addr, "login", None, json!({ "id": "ann", "password": PW })).await;
    let (s, v) = post(&addr, "totp", None, json!({ "pending": v["pending"], "code": c })).await;
    assert_eq!((s, v["error"].as_str()), (401, Some("invalid_code")));
    assert_eq!(post(&addr, "totp/disable", Some(&t), json!({ "password": "wrong password" })).await.0, 401);
    assert_eq!(post(&addr, "totp/disable", Some(&t), json!({ "password": PW })).await.0, 200);
    login(&addr, "ann").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_native_client_signs_up_in_and_out_over_a_pinned_link() {
    use aio::auth::client::{Client, Login, Me, Signup};
    let roots = tempfile::tempdir().unwrap();
    let tls = aio::tls::server(&roots.path().join("a"), "board").unwrap();
    let root = tls.root_pem.clone();
    let h = Handle::<Board>::memory();
    let auth = Arc::new(Auth::memory(Policy { signup: true, ..Default::default() }).unwrap());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = Options { token: "secret".into(), auth: Some(auth.clone()), tls: Some(tls), ..Default::default() };
    tokio::spawn(serve_with(h.clone(), l, opts));
    let c = Client::new(&addr, Some(root));
    assert_eq!(c.me(None).await.unwrap(), Me { user: None, signup: true, totp: true, mail: false });
    let Signup::Signed(ann) = c.signup("ann", PW, None).await.unwrap() else { panic!("signed in") };
    assert_eq!((ann.user.as_str(), ann.role.as_str()), ("ann", "user"));
    let e = c.signup("ann", PW, None).await.unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.status, e.to_string().as_str()),
        ("user_exists", 409, "That username is already taken.")
    );
    assert_eq!(c.me(Some(&ann.token)).await.unwrap().user, Some(("ann".into(), "user".into())));
    // The session is the link's key: a copy as ann.
    let copy = Remote::<Board>::connect_to(c.link(&ann), "ann").await.unwrap();
    copy.add("signed".into()).unwrap();
    until("as ann", || h.state().last == Some(By::User("ann".into()))).await;
    // A new password ends the old session and hands a new one.
    let new = "a new horse battery";
    let ann2 = c.password(&ann.token, PW, new).await.unwrap();
    assert_eq!(c.me(Some(&ann.token)).await.unwrap().user, None);
    let e = c.login("ann", PW).await.unwrap_err();
    assert_eq!(e.to_string(), "Incorrect username or password.");
    assert!(matches!(c.login("ann", new).await.unwrap(), Login::Signed(s) if s.user == "ann"));
    // A second factor: set up, turned on with the password, then asked for.
    let (secret, uri) = c.totp_setup(&ann2.token).await.unwrap();
    assert!(uri.starts_with("otpauth://totp/"));
    // One step read once: the server takes its step ± one, and a hash takes
    // seconds, so a code read per call could fall out of it at a step's end.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    let step = now - now % aio::auth::totp::STEP_MS;
    c.totp_enable(&ann2.token, &aio::testing::totp_code(&secret, step), new).await.unwrap();
    let Login::Totp { pending } = c.login("ann", new).await.unwrap() else { panic!("a second step") };
    let e = c.totp(&pending, "000000").await.unwrap_err();
    assert_eq!(e.to_string(), "That code is not correct.");
    let Login::Totp { pending } = c.login("ann", new).await.unwrap() else { panic!("a second step") };
    let s = c.totp(&pending, &aio::testing::totp_code(&secret, step + aio::auth::totp::STEP_MS)).await.unwrap();
    c.logout(&s.token).await.unwrap();
    assert_eq!(c.me(Some(&s.token)).await.unwrap().user, None);
    c.totp_disable(&ann2.token, new).await.unwrap();
    // A link that pins another root reaches nothing.
    let other = aio::tls::server(&roots.path().join("b"), "board").unwrap().root_pem;
    let e = Client::new(&addr, Some(other)).me(None).await.unwrap_err();
    assert_eq!((e.code.as_str(), e.status), ("unreachable", 0));
}

type Sent = Arc<std::sync::Mutex<Vec<aio::auth::Mail>>>;

/// A server with accounts whose mail lands in the list it returns (`fail`: every send fails).
async fn mailing(policy: Policy, fail: bool) -> (Arc<Auth>, String, Sent) {
    let auth = Arc::new(Auth::memory(policy).unwrap());
    let sent: Sent = Default::default();
    let s2 = sent.clone();
    let http = aio::http::Http::default().mail(move |m| {
        s2.lock().unwrap().push(m.clone());
        if fail { Err("smtp down".into()) } else { Ok(()) }
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = Options { token: "secret".into(), auth: Some(auth.clone()), http, ..Default::default() };
    tokio::spawn(serve_with(Handle::<Board>::memory(), l, opts));
    (auth, addr, sent)
}

/// The token in the last mail to `to`.
fn token_to(sent: &Sent, to: &str) -> String {
    let m = sent.lock().unwrap().iter().rev().find(|m| m.to == to).cloned().unwrap();
    m.text.split_whitespace().find(|w| w.starts_with("aiot_")).unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_mail_hook_the_mail_doors_say_so() {
    let (_h, _auth, addr) = server(Policy::default()).await;
    let (_, _, me) = http(&addr, "GET", "/__aio/auth/me", None, json!(null)).await;
    assert_eq!(me["features"]["mail"], false);
    let t = login(&addr, "ann").await;
    for (op, bearer) in [("verify/request", Some(t.as_str())), ("verify/resend", None), ("reset/request", None)] {
        assert_eq!(
            post(&addr, op, bearer, json!({ "id": "ann" })).await,
            (501, json!({ "error": "mail_not_configured" }))
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_verified_email_comes_by_a_mailed_token_that_works_once() {
    let (auth, addr, sent) = mailing(Policy { signup: true, ..Default::default() }, false).await;
    let (_, _, me) = http(&addr, "GET", "/__aio/auth/me", None, json!(null)).await;
    assert_eq!(me["features"]["mail"], true);
    let bad = json!({ "id": "cy", "password": PW, "email": "not-an-address" });
    assert_eq!(post(&addr, "signup", None, bad).await, (400, json!({ "error": "invalid_email" })));
    // Without require_verified a sign-up signs in at once, and the mail still goes out.
    let (s, v) = post(&addr, "signup", None, json!({ "id": "cy", "password": PW, "email": "cy@example.com" })).await;
    assert_eq!((s, v["user"]["id"].as_str(), v["token"].is_string()), (201, Some("cy"), true));
    until("the mail", || sent.lock().unwrap().len() == 1).await;
    let m = sent.lock().unwrap()[0].clone();
    assert_eq!((m.to.as_str(), m.subject.as_str()), ("cy@example.com", "Verify your board account"));
    let token = token_to(&sent, "cy@example.com");
    assert!(!auth.user("cy").unwrap().unwrap().verified);
    assert_eq!(post(&addr, "verify", None, json!({ "token": token })).await.0, 200);
    assert!(auth.user("cy").unwrap().unwrap().verified);
    assert_eq!(
        post(&addr, "verify", None, json!({ "token": token })).await,
        (401, json!({ "error": "invalid_or_expired_token" }))
    );
    assert_eq!(post(&addr, "verify", None, json!({})).await.1, json!({ "error": "token_required" }));
    // Signed in: ask for a new one; verified already, or no address, is said.
    let t = v["token"].as_str().unwrap();
    assert_eq!(
        post(&addr, "verify/request", Some(t), json!({})).await.1,
        json!({ "ok": true, "alreadyVerified": true })
    );
    assert_eq!(post(&addr, "verify/request", None, json!({})).await.0, 401);
    auth.create("dee", PW, None).unwrap();
    let d = login(&addr, "dee").await;
    assert_eq!(
        post(&addr, "verify/request", Some(&d), json!({})).await,
        (400, json!({ "error": "no_email_on_account" }))
    );
    auth.set_email("dee", "dee@example.com").unwrap();
    assert_eq!(post(&addr, "verify/request", Some(&d), json!({})).await, (200, json!({ "ok": true })));
    assert_eq!(post(&addr, "verify", None, json!({ "token": token_to(&sent, "dee@example.com") })).await.0, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn with_verification_required_no_one_signs_in_before_their_email_is_proven() {
    let policy = Policy { signup: true, require_verified: true, ..Default::default() };
    let (auth, addr, sent) = mailing(policy.clone(), false).await;
    assert_eq!(
        post(&addr, "signup", None, json!({ "id": "cy", "password": PW })).await,
        (400, json!({ "error": "email_required" }))
    );
    let (s, v) = post(&addr, "signup", None, json!({ "id": "cy", "password": PW, "email": "cy@example.com" })).await;
    assert_eq!((s, v), (201, json!({ "user": { "id": "cy", "role": "user" }, "verificationSent": true })));
    let refused = post(&addr, "login", None, json!({ "id": "cy", "password": PW })).await;
    assert_eq!(refused, (403, json!({ "error": "email_unverified" })));
    // The token expired, or was lost: the anonymous door sends a new one, and answers the same for anyone.
    let first = token_to(&sent, "cy@example.com");
    for id in ["cy", "nobody"] {
        assert_eq!(post(&addr, "verify/resend", None, json!({ "id": id })).await, (200, json!({ "ok": true })));
    }
    until("a second mail", || sent.lock().unwrap().len() == 2).await;
    assert_eq!(sent.lock().unwrap().iter().filter(|m| m.to == "cy@example.com").count(), 2, "none for a miss");
    let second = token_to(&sent, "cy@example.com");
    assert_ne!(first, second);
    assert_eq!(post(&addr, "verify", None, json!({ "token": second })).await.0, 200);
    login(&addr, "cy").await;
    // A verified account gets no more mail from the anonymous door.
    assert_eq!(post(&addr, "verify/resend", None, json!({ "id": "cy" })).await.0, 200);
    // A sign-up whose mail failed still made the account, and says the mail did not go.
    let (auth2, addr2, _) = mailing(policy, true).await;
    let (s, v) = post(&addr2, "signup", None, json!({ "id": "ed", "password": PW, "email": "ed@example.com" })).await;
    assert_eq!((s, &v["verificationSent"]), (201, &json!(false)));
    assert!(auth2.user("ed").unwrap().is_some());
    // The operator's verify opens the door too.
    auth2.mark_verified("ed").unwrap();
    login(&addr2, "ed").await;
    drop(auth);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reset_by_mail_tells_a_prober_nothing_and_rescues_the_account_from_everyone() {
    let (auth, addr, sent) = mailing(Policy::default(), false).await;
    auth.create("ann", PW, None).unwrap();
    auth.set_email("ann", "ann@example.com").unwrap();
    let old = login(&addr, "ann").await;
    for _ in 0..5 {
        _ = post(&addr, "login", None, json!({ "id": "ann", "password": "wrong password" })).await;
    }
    assert_eq!(post(&addr, "login", None, json!({ "id": "ann", "password": PW })).await.0, 423, "locked");
    for id in ["ann", "nobody", " ann"] {
        assert_eq!(post(&addr, "reset/request", None, json!({ "id": id })).await, (200, json!({ "ok": true })), "{id}");
    }
    until("a mail per ask for ann", || sent.lock().unwrap().len() == 2).await;
    let m = sent.lock().unwrap()[0].clone();
    assert_eq!((m.to.as_str(), m.subject.as_str()), ("ann@example.com", "Reset your board password"));
    let token = token_to(&sent, "ann@example.com");
    // A too-short password is a typo: the token survives it.
    let short = post(&addr, "reset", None, json!({ "token": token, "password": "short" })).await;
    assert_eq!(short, (400, json!({ "error": "password_too_short" })));
    let new = "a brand new horse";
    assert_eq!(
        post(&addr, "reset", None, json!({ "token": token, "password": new })).await,
        (200, json!({ "ok": true }))
    );
    assert_eq!(post(&addr, "reset", None, json!({ "token": token, "password": new })).await.0, 401, "once");
    assert_eq!(post(&addr, "reset", None, json!({})).await.1, json!({ "error": "token_and_password_required" }));
    // Every old session is gone, the lock with them, and the mailbox is proven.
    assert_eq!(http(&addr, "GET", "/__aio/auth/me", Some(&old), json!(null)).await.2["user"], json!(null));
    let (s, _) = post(&addr, "login", None, json!({ "id": "ann", "password": new })).await;
    assert_eq!(s, 200);
    assert!(auth.user("ann").unwrap().unwrap().verified);
    // An inbox is no mail-bomb target: past the budget the door answers the same and sends nothing.
    let before = sent.lock().unwrap().len();
    for _ in 0..12 {
        assert_eq!(post(&addr, "reset/request", None, json!({ "id": "ann" })).await.0, 200);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(sent.lock().unwrap().len() - before <= 8, "budgeted: {}", sent.lock().unwrap().len() - before);
}

/// Each user's own pages: erased only by their author, seen by them and an editor.
#[cell(persist, shared, visible = own)]
pub struct Book {
    pub pages: BTreeMap<Uuid, (By, String)>,
}

fn own(b: &Book, viewer: &Caller) -> Book {
    let editor = viewer.role.as_deref() == Some("editor");
    let pages = b.pages.iter().filter(|(_, (by, _))| editor || *by == viewer.by);
    Book { pages: pages.map(|(k, v)| (*k, v.clone())).collect() }
}

fn author(b: &Book, who: &Caller, id: Uuid) -> bool {
    b.pages.get(&id).is_some_and(|(by, _)| *by == who.by)
}

#[methods]
impl Book {
    #[client]
    pub fn write(&mut self, cx: &Cx, text: String) -> Result<Uuid> {
        let id = cx.id();
        self.pages.insert(id, (cx.by().clone(), text));
        Ok(id)
    }

    #[client(allow = author)]
    pub fn erase(&mut self, id: Uuid) {
        self.pages.remove(&id);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn with_accounts_sync_sees_and_changes_only_what_the_signed_user_may() {
    let h = Handle::<Book>::memory();
    let auth = Arc::new(Auth::memory(Policy::default()).unwrap());
    auth.create("ann", PW, None).unwrap();
    auth.create("bob", PW, None).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = Options { token: "secret".into(), auth: Some(auth.clone()), ..Default::default() };
    tokio::spawn(serve_with(h.clone(), l, opts));
    let ann = Remote::<Book>::connect(&addr, &login(&addr, "ann").await, "ann").await.unwrap();
    let bob = Remote::<Book>::connect(&addr, &login(&addr, "bob").await, "bob").await.unwrap();
    let mine = ann.write("ann's".into()).unwrap();
    bob.write("bob's".into()).unwrap();
    until("both written", || h.state().pages.len() == 2 && ann.pending() == 0 && bob.pending() == 0).await;
    until("each sees one", || ann.confirmed().pages.len() == 1 && bob.confirmed().pages.len() == 1).await;
    assert!(ann.confirmed().pages.contains_key(&mine) && !bob.confirmed().pages.contains_key(&mine));
    // Bob can't erase Ann's page, even knowing its id: the rule sees the session's user.
    let said = Arc::new(std::sync::Mutex::new(String::new()));
    let s2 = said.clone();
    bob.on_refused(move |m, e| *s2.lock().unwrap() = format!("{m}: {e}"));
    bob.erase(mine).unwrap();
    until("refused", || !said.lock().unwrap().is_empty()).await;
    assert!(said.lock().unwrap().starts_with("erase: "), "{}", said.lock().unwrap());
    assert!(h.state().pages.contains_key(&mine));
    // What a link sees goes by its role, a new one at once: Bob made an editor sees Ann's page.
    auth.set_role("bob", "editor").unwrap();
    until("bob sees both", || bob.confirmed().pages.len() == 2).await;
    auth.set_role("bob", "user").unwrap();
    until("and back to his own", || bob.confirmed().pages.len() == 1).await;
    ann.erase(mine).unwrap();
    until("erased by its author", || !h.state().pages.contains_key(&mine)).await;
}

#[cfg(feature = "compat")]
#[tokio::test(flavor = "multi_thread")]
async fn with_accounts_an_aio_page_sees_what_its_role_may_and_follows_a_role_change() {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    let h = Handle::<Book>::memory();
    let auth = Arc::new(Auth::memory(Policy::default()).unwrap());
    auth.create("ann", PW, None).unwrap();
    auth.create("bob", PW, None).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = Options { token: "secret".into(), auth: Some(auth.clone()), ..Default::default() };
    tokio::spawn(serve_with(h.clone(), l, opts));
    let ann = Remote::<Book>::connect(&addr, &login(&addr, "ann").await, "ann").await.unwrap();
    ann.write("ann's".into()).unwrap();
    until("written", || h.state().pages.len() == 1 && ann.pending() == 0).await;
    // Bob's aio page, signed in by its session header.
    let mut req = format!("ws://{addr}/ws").into_client_request().unwrap();
    let bearer = format!("Bearer {}", login(&addr, "bob").await);
    req.headers_mut().insert("authorization", bearer.parse().unwrap());
    let (mut w, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let mut states = async move || loop {
        let m = tokio::time::timeout(Duration::from_secs(5), w.next()).await.unwrap().unwrap().unwrap();
        let Message::Text(t) = m else { continue };
        let v: Value = serde_json::from_str(&t).unwrap();
        if v["t"] == "state" {
            return v["d"]["book"]["pages"].as_object().unwrap().len();
        }
    };
    assert_eq!(states().await, 0, "not his page");
    auth.set_role("bob", "editor").unwrap();
    assert_eq!(states().await, 1, "an editor sees ann's page at once");
    auth.set_role("bob", "user").unwrap();
    assert_eq!(states().await, 0, "and back");
}

#[tokio::test(flavor = "multi_thread")]
async fn with_accounts_health_and_metrics_need_the_key_or_a_session() {
    let (_h, _auth, addr) = server(Policy::default()).await;
    for path in ["/__aio/health", "/__aio/metrics"] {
        assert_eq!(http(&addr, "GET", path, None, json!(null)).await.0, 401, "{path}");
        assert_eq!(http(&addr, "GET", path, Some("nope"), json!(null)).await.0, 401, "{path}");
        assert_eq!(http(&addr, "GET", path, Some("secret"), json!(null)).await.0, 200, "{path}");
    }
    let t = login(&addr, "bob").await;
    let (s, _, v) = http(&addr, "GET", "/__aio/health", Some(&t), json!(null)).await;
    assert_eq!((s, v["appId"].as_str()), (200, Some("board")));
}

#[tokio::test(flavor = "multi_thread")]
async fn with_accounts_a_session_opens_routes_as_its_user() {
    use aio::http::{Http, Reply, Route};
    let auth = Arc::new(Auth::memory(Policy::default()).unwrap());
    auth.create("ann", PW, Some("admin")).unwrap();
    let who = |r: aio::http::Req| async move { Reply::json(&json!({ "user": r.user(), "role": r.role() })) };
    let routes = Http::default().route(Route::get("/whoami", who)).route(Route::get("/open", who).public());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = Options { token: "secret".into(), auth: Some(auth), http: routes, ..Default::default() };
    tokio::spawn(serve_with(Handle::<Board>::memory(), l, opts));
    assert_eq!(http(&addr, "GET", "/whoami", None, json!(null)).await.0, 401);
    assert_eq!(http(&addr, "GET", "/whoami", Some("not-a-session"), json!(null)).await.0, 401);
    let t = login(&addr, "ann").await;
    let (s, _, v) = http(&addr, "GET", "/whoami", Some(&t), json!(null)).await;
    assert_eq!((s, v), (200, json!({ "user": "ann", "role": "admin" })));
    // The key still opens routes, as no one; a public route still knows a session.
    assert_eq!(
        http(&addr, "GET", "/whoami", Some("secret"), json!(null)).await.2,
        json!({ "user": null, "role": null })
    );
    assert_eq!(http(&addr, "GET", "/open", None, json!(null)).await.2["user"], json!(null));
    assert_eq!(http(&addr, "GET", "/open", Some(&t), json!(null)).await.2["user"], "ann");
    // A session that ends opens nothing more.
    assert_eq!(post(&addr, "logout", Some(&t), json!({})).await.0, 200);
    assert_eq!(http(&addr, "GET", "/whoami", Some(&t), json!(null)).await.0, 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn with_accounts_the_shell_loads_before_sign_in_and_dirs_still_need_one() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("uploads")).unwrap();
    std::fs::write(t.path().join("index.html"), "<html>sign in</html>").unwrap();
    std::fs::write(t.path().join("uploads/x.png"), "private").unwrap();
    let auth = Arc::new(Auth::memory(Policy::default()).unwrap());
    auth.create("ann", PW, None).unwrap();
    let blobs = aio::blobs::Blobs::at(t.path().join("blobs"));
    let blob = aio::blobs::url(&blobs.put(&b"secret bytes"[..], None).unwrap().id);
    let routes = aio::http::Http::default().shell(t.path()).dir("/uploads", t.path().join("uploads")).blobs(blobs);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = Options { token: "secret".into(), auth: Some(auth), http: routes, ..Default::default() };
    tokio::spawn(serve_with(Handle::<Board>::memory(), l, opts));
    for p in ["/", "/notes/today"] {
        assert_eq!(http(&addr, "GET", p, None, json!(null)).await.0, 200, "{p}");
    }
    // The shell is public; files and blob bytes are not.
    for p in ["/uploads/x.png", &blob] {
        assert_eq!(http(&addr, "GET", p, None, json!(null)).await.0, 401, "{p}");
    }
    let t = login(&addr, "ann").await;
    for p in ["/uploads/x.png", &blob] {
        assert_eq!(http(&addr, "GET", p, Some(&t), json!(null)).await.0, 200, "{p}");
    }
}

/// One plain HTTP/1.1 request with any headers; the status, the lower-cased head and the JSON body.
async fn raw(addr: &str, method: &str, path: &str, headers: &[(&str, &str)], body: Value) -> (u16, String, Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = body.to_string();
    let extra: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    (head[9..12].parse().unwrap(), head.to_ascii_lowercase(), serde_json::from_str(body).unwrap_or_default())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_keeps_its_session_in_a_strict_httponly_cookie_secure_behind_a_trusted_proxy() {
    use aio::http::{Http, Reply, Route};
    let auth = Arc::new(Auth::memory(Policy::default()).unwrap());
    auth.create("ann", PW, None).unwrap();
    let who = Http::default().route(Route::get("/whoami", |r| async move { Reply::text(r.user().unwrap_or("-")) }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = Options {
        token: "secret".into(),
        auth: Some(auth),
        http: who,
        trust_proxy: Some("x-forwarded-for".into()),
        ..Default::default()
    };
    tokio::spawn(serve_with(Handle::<Board>::memory(), l, opts));
    let creds = json!({ "id": "ann", "password": PW });
    let (s, head, v) = raw(&addr, "POST", "/__aio/auth/login", &[], creds.clone()).await;
    assert_eq!(s, 200);
    let line = head.lines().find(|l| l.starts_with("set-cookie: aio_session_board=")).expect(&head).to_string();
    for part in ["; path=/", "; max-age=", "; httponly", "; samesite=strict"] {
        assert!(line.contains(part), "{part} in {line}");
    }
    assert!(!line.contains("; secure"), "plain http, no proxy: {line}");
    let token = v["token"].as_str().unwrap().to_string();
    assert!(line.contains(&token.to_ascii_lowercase()), "{line}");
    let https = [("X-Forwarded-For", "203.0.113.7"), ("X-Forwarded-Proto", "https")];
    let (_, head, _) = raw(&addr, "POST", "/__aio/auth/login", &https, creds).await;
    assert!(head.lines().any(|l| l.starts_with("set-cookie: aio_session_board=") && l.contains("; secure")), "{head}");
    // The cookie opens what a bearer opens, then signs out (and is cleared).
    let cookie = format!("aio_session_board={token}");
    let jar = [("Cookie", cookie.as_str())];
    assert_eq!(raw(&addr, "GET", "/__aio/auth/me", &jar, json!(null)).await.2["user"]["id"], "ann");
    assert_eq!(raw(&addr, "GET", "/whoami", &jar, json!(null)).await.0, 200, "a route knows the cookie too");
    let (s, head, _) = raw(&addr, "POST", "/__aio/auth/logout", &jar, json!({})).await;
    assert_eq!(s, 200);
    assert!(
        head.lines().any(|l| l.starts_with("set-cookie: aio_session_board=;") && l.contains("max-age=0")),
        "{head}"
    );
    assert_eq!(raw(&addr, "GET", "/__aio/auth/me", &jar, json!(null)).await.2["user"], json!(null));
    assert_eq!(raw(&addr, "GET", "/whoami", &jar, json!(null)).await.0, 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn behind_a_trusted_proxy_sign_in_budgets_are_the_clients() {
    let auth = Arc::new(Auth::memory(Policy::default()).unwrap());
    auth.create("ann", PW, None).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let opts = Options {
        token: "secret".into(),
        auth: Some(auth),
        trust_proxy: Some("x-forwarded-for".into()),
        ..Default::default()
    };
    tokio::spawn(serve_with(Handle::<Board>::memory(), l, opts));
    let wrong = json!({ "id": "ann", "password": "not it at all" });
    let a = [("X-Forwarded-For", "198.51.100.1")];
    let mut last = 0;
    for _ in 0..12 {
        last = raw(&addr, "POST", "/__aio/auth/login", &a, wrong.clone()).await.0;
    }
    assert_eq!(last, 429, "one client spent its budget");
    // Another client behind the same proxy has its own.
    let b = [("X-Forwarded-For", "198.51.100.2")];
    assert_eq!(raw(&addr, "POST", "/__aio/auth/login", &b, wrong).await.0, 401);
}

async fn served(http: aio::http::Http) -> (Handle<Board>, String) {
    let h = Handle::<Board>::memory();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(serve_with(h.clone(), l, Options { token: "secret".into(), http, ..Default::default() }));
    (h, addr)
}

/// aio's `users:`/`resolveUser` re-check: an open socket's token is asked
/// again every 5 s, and one no longer resolved (revoked, a resolver that
/// panics, a changed role) closes it, for good.
#[tokio::test(flavor = "multi_thread")]
async fn a_users_token_is_rechecked_on_its_open_socket() {
    use std::sync::atomic::{AtomicU8, Ordering};
    // 0: ann is a user; 1: her token is revoked; 2: the resolver panics; 3: ann is an admin now.
    let mode = Arc::new(AtomicU8::new(0));
    let m = mode.clone();
    let users = aio::auth::Users::resolve(move |t| match (t, m.load(Ordering::SeqCst)) {
        ("tok-ann-0123456789abcdef", 2) => panic!("resolver bug"),
        ("tok-ann-0123456789abcdef", 0) => Some(("ann".into(), "user".into())),
        ("tok-ann-0123456789abcdef", 3) => Some(("ann".into(), "admin".into())),
        ("tok-bob-0123456789abcdef", _) => Some(("bob".into(), "user".into())),
        _ => None,
    });
    let (h, addr) = served(aio::http::Http::default().users(users)).await;
    let gone = |r: &Remote<Board>| {
        let r = r.clone();
        async move {
            for _ in 0..1_000 {
                if !r.online() {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            false
        }
    };
    let bob = Remote::<Board>::connect(&addr, "tok-bob-0123456789abcdef", "bob").await.unwrap();
    for (end, what) in [(1, "revoked"), (2, "a panicking resolver")] {
        mode.store(0, Ordering::SeqCst);
        let ann = Remote::<Board>::connect(&addr, "tok-ann-0123456789abcdef", "ann").await.unwrap();
        ann.add("hi".into()).unwrap();
        until("ann's call is in", || h.state().last == Some(By::User("ann".into()))).await;
        mode.store(end, Ordering::SeqCst);
        assert!(gone(&ann).await, "{what}: the socket closes within the re-check");
        assert!(bob.online(), "{what}: only that token's socket");
    }
    // A changed role: the socket closes, and the client's reconnect calls with the new one.
    mode.store(0, Ordering::SeqCst);
    let ann = Remote::<Board>::connect(&addr, "tok-ann-0123456789abcdef", "ann").await.unwrap();
    ann.add("hi".into()).unwrap();
    until("ann's call is in", || h.state().role.as_deref() == Some("user")).await;
    mode.store(3, Ordering::SeqCst);
    for _ in 0..100 {
        if h.state().role.as_deref() == Some("admin") {
            break;
        }
        ann.add("again".into()).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(h.state().role.as_deref(), Some("admin"), "the new role, not the one it opened with");
}

/// Sessions without accounts (aio's `sessions: true`): the app issues a
/// session, which is its user on the socket; no login doors are served; a
/// revoked session's socket closes at once.
#[tokio::test(flavor = "multi_thread")]
async fn sessions_without_accounts() {
    let auth = Arc::new(Auth::memory(Policy::default()).unwrap());
    let known = aio::auth::Users::map([("svc-0123456789abcdef0123".into(), ("svc".into(), "service".into()))]);
    let users = aio::auth::Users::sessions(auth.clone(), Some(known));
    let (h, addr) = served(aio::http::Http::default().users(users)).await;
    let t = auth.issue("carol", "user").unwrap();
    let carol = Remote::<Board>::connect(&addr, &t, "carol").await.unwrap();
    carol.add("hi".into()).unwrap();
    until("carol's call is in", || h.state().last == Some(By::User("carol".into()))).await;
    assert_eq!(h.state().role.as_deref(), Some("user"));
    // The app's own users compose, after the sessions.
    let svc = Remote::<Board>::connect(&addr, "svc-0123456789abcdef0123", "svc").await.unwrap();
    // No accounts: no login doors, and the key opens nothing.
    assert_eq!(post(&addr, "login", None, json!({ "id": "carol", "password": PW })).await.0, 404);
    let e = Remote::<Board>::connect(&addr, "secret", "x").await.err().unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(auth.revoke(&t).unwrap());
    until("closed on revoke", || !carol.online()).await;
    assert!(svc.online());
    let e = Remote::<Board>::connect(&addr, &t, "carol").await.err().unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
}

//! The ready sign-in form, driven like a person against a real app.
use aio::auth::client::{Client, Signed};
use aio::auth::{Auth, Policy};
use aio::kit::SignIn;
use aio::prelude::*;
use aio::testing::TestUi;
use aio::ui::prelude::*;
use std::cell::RefCell;
use std::sync::Arc;

#[cell(persist, shared)]
pub struct Desk {
    pub n: u64,
}

#[methods]
impl Desk {}

const PW: &str = "correct horse battery";

thread_local! {
    static AT: RefCell<String> = const { RefCell::new(String::new()) };
    static SIGNED: RefCell<Option<Signed>> = const { RefCell::new(None) };
}

type Sent = Arc<std::sync::Mutex<Vec<aio::auth::Mail>>>;

/// An app with accounts (`ann`), served on aio's runtime; this thread's
/// form signs in to it. With `mail`, what it mails lands in the list.
fn serve(policy: Policy, mail: bool) -> (Arc<Auth>, Sent) {
    let auth = Arc::new(Auth::memory(policy).unwrap());
    auth.create("ann", PW, None).unwrap();
    let sent: Sent = Default::default();
    let s2 = sent.clone();
    let http = aio::http::Http::default();
    let http = if mail {
        http.mail(move |m| {
            s2.lock().unwrap().push(m.clone());
            Ok(())
        })
    } else {
        http
    };
    let l = aio::rt().block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    AT.set(l.local_addr().unwrap().to_string());
    let opts = aio::sync::Options { token: "secret".into(), auth: Some(auth.clone()), http, ..Default::default() };
    aio::rt().spawn(aio::sync::serve_with(Handle::<Desk>::memory(), l, opts));
    SIGNED.set(None);
    (auth, sent)
}

fn app(signup: bool) -> Arc<Auth> {
    serve(Policy { signup, ..Default::default() }, false).0
}

fn form() -> View {
    let client = Client::new(AT.with_borrow(Clone::clone), None);
    rsx! { SignIn { client, on_signed: move |s| SIGNED.set(Some(s)) } }
}

fn signed() -> Option<String> {
    SIGNED.with_borrow(|s| s.as_ref().map(|s| s.user.clone()))
}

/// Wait until `t` is there and shows `want` (an answer comes off-thread).
fn shows(ui: &mut TestUi, t: &str, want: &str) {
    for _ in 0..500 {
        if ui.present(t) && ui.text(t) == want {
            return;
        }
        ui.wait(std::time::Duration::from_millis(10));
    }
    ui.until_text(t, want);
}

fn enter(ui: &mut TestUi, id: &str, pw: &str) {
    ui.set_value("signinId", id);
    ui.set_value("signinPassword", pw);
    ui.click("signinSubmit");
}

#[test]
fn a_person_signs_in_and_a_wrong_password_is_said_in_words() {
    app(false);
    let mut ui = TestUi::new(form);
    assert_eq!(ui.attr("signinPassword", "autocomplete").as_deref(), Some("current-password"));
    assert_eq!(ui.text("signinSubmit"), "Sign in");
    enter(&mut ui, "ann", "wrong password");
    shows(&mut ui, "signinError", "Incorrect username or password.");
    assert_eq!(signed(), None);
    ui.set_value("signinPassword", PW);
    ui.click("signinSubmit");
    ui.until("signed in", || signed().as_deref() == Some("ann"));
    assert!(ui.absent("signinError"));
    // Sign-up is off on this server: no toggle.
    assert!(ui.absent("signinToggle"));
}

#[test]
fn sign_up_shows_when_the_server_allows_it_and_signs_in_the_new_account() {
    app(true);
    let mut ui = TestUi::new(form);
    shows(&mut ui, "signinToggle", "New here? Create an account");
    ui.click("signinToggle");
    assert_eq!(ui.text("signinSubmit"), "Create account");
    assert_eq!(ui.attr("signinPassword", "autocomplete").as_deref(), Some("new-password"));
    enter(&mut ui, "ann", PW);
    shows(&mut ui, "signinError", "That username is already taken.");
    ui.type_text("signinId", "-bo");
    ui.click("signinSubmit");
    ui.until("bob is in", || signed().is_some());
    assert_eq!(signed().as_deref(), Some("ann-bo"));
}

#[test]
fn a_second_factor_asks_for_the_code_and_a_wrong_one_starts_again() {
    let auth = app(false);
    let secret = auth.totp_setup("ann").unwrap();
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    auth.totp_enable("ann", &aio::testing::totp_code(&secret, now() - aio::auth::totp::STEP_MS)).unwrap();
    let mut ui = TestUi::new(form);
    enter(&mut ui, "ann", PW);
    shows(&mut ui, "signinSubmit", "Verify");
    assert!(ui.absent("signinPassword"), "the password is not asked again");
    assert_eq!(signed(), None, "not in before the second factor");
    ui.type_text("signinCode", "000000");
    ui.click("signinSubmit");
    shows(&mut ui, "signinError", "Wrong code — sign in again.");
    assert!(ui.absent("signinCode"), "the spent step keeps no form");
    enter(&mut ui, "ann", PW);
    shows(&mut ui, "signinSubmit", "Verify");
    ui.type_text("signinCode", &aio::testing::totp_code(&secret, now()));
    ui.click("signinSubmit");
    ui.until("signed in", || signed().as_deref() == Some("ann"));
}

#[test]
fn a_forgotten_password_is_reset_by_a_mailed_token_when_the_app_sends_mail() {
    let (auth, sent) = serve(Policy::default(), true);
    auth.set_email("ann", "ann@example.com").unwrap();
    let mut ui = TestUi::new(form);
    shows(&mut ui, "signinForgot", "Forgot password?");
    ui.click("signinForgot");
    assert!(ui.absent("signinPassword"), "asking for a token needs only the id");
    ui.set_value("signinId", "ann");
    ui.click("signinSubmit");
    shows(
        &mut ui,
        "signinNotice",
        "If that account has an email on file, a reset token is on its way. Paste it below with your new password.",
    );
    ui.until("the mail", || !sent.lock().unwrap().is_empty());
    let text = sent.lock().unwrap()[0].text.clone();
    let token = text.split_whitespace().find(|w| w.starts_with("aiot_")).unwrap().to_string();
    // A wrong token says so and keeps the form.
    ui.set_value("signinToken", "aiot_nope");
    ui.set_value("signinPassword", "a brand new horse");
    ui.click("signinSubmit");
    shows(&mut ui, "signinError", "That reset token is used up or expired — ask for a new one.");
    assert!(ui.present("signinToken"));
    ui.set_value("signinToken", &token);
    ui.click("signinSubmit");
    shows(&mut ui, "signinNotice", "Password changed — sign in with it now.");
    assert_eq!(signed(), None, "a reset is no session");
    enter(&mut ui, "ann", "a brand new horse");
    ui.until("signed in with the new password", || signed().as_deref() == Some("ann"));
}

#[test]
fn without_mail_there_is_no_forgotten_password_door_and_back_clears_the_error() {
    let (_auth, _) = serve(Policy::default(), false);
    let mut ui = TestUi::new(form);
    enter(&mut ui, "ann", "wrong password");
    shows(&mut ui, "signinError", "Incorrect username or password.");
    assert!(ui.absent("signinForgot"), "no mail: no door that can't work");
    let (_auth, _) = serve(Policy::default(), true);
    let mut ui = TestUi::new(form);
    shows(&mut ui, "signinForgot", "Forgot password?");
    ui.click("signinForgot");
    ui.click("signinHaveToken");
    ui.set_value("signinToken", "aiot_nope");
    ui.set_value("signinPassword", "a brand new horse");
    ui.click("signinSubmit");
    shows(&mut ui, "signinError", "That reset token is used up or expired — ask for a new one.");
    ui.click("signinBack");
    assert!(ui.absent("signinError") && ui.absent("signinNotice"));
    assert_eq!(ui.attr("signinPassword", "autocomplete").as_deref(), Some("current-password"));
}

#[test]
fn a_sign_up_that_needs_verification_says_so_instead_of_signing_in() {
    let (auth, sent) = serve(Policy { signup: true, require_verified: true, ..Default::default() }, true);
    let mut ui = TestUi::new(form);
    shows(&mut ui, "signinToggle", "New here? Create an account");
    ui.click("signinToggle");
    ui.set_value("signinId", "cy");
    ui.set_value("signinEmail", "cy@example.com");
    ui.set_value("signinPassword", PW);
    ui.click("signinSubmit");
    shows(&mut ui, "signinNotice", "Account created — check your inbox to verify, then sign in.");
    assert_eq!((signed(), sent.lock().unwrap().len()), (None, 1));
    assert_eq!(ui.text("signinSubmit"), "Sign in", "back at sign-in");
    enter(&mut ui, "cy", PW);
    shows(
        &mut ui,
        "signinError",
        "This account's email address has not been verified yet. Check your inbox for the verification link.",
    );
    auth.mark_verified("cy").unwrap();
    ui.click("signinSubmit");
    ui.until("signed in once verified", || signed().as_deref() == Some("cy"));
}

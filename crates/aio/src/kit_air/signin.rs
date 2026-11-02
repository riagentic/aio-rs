//! A ready sign-in form (aio's `<SignIn/>`): sign in, create an account when
//! the app allows it, the second-factor step, and, when the app sends mail,
//! a forgotten password (ask for a token, then set a new password with it),
//! over [`crate::auth::client`]. It adapts to the server (no sign-up toggle
//! when sign-up is off, no "Forgot password?" without mail) and says each
//! refusal in words a person can act on. A wrong second-factor code spends
//! the step: back to the password.
use super::{Alert, Button, Field, Input, Tone, Variant, join};
use crate::auth::client::{AuthError, Client, Login, Me, Signed, Signup};
use crate::ui::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Login,
    Signup,
    /// Ask for a reset token by mail.
    Forgot,
    /// Set a new password with it.
    Reset,
}

/// What a submit led to.
enum Done {
    Signed(Signed),
    Totp(String),
    /// Back to the login face, saying this.
    Told(&'static str),
    /// The token is on its way: its form next.
    Asked,
}

/// `f` on aio's runtime, where its socket lives; a page asks on its own tasks.
#[cfg(not(target_arch = "wasm32"))]
async fn asked<R: Send + 'static>(
    f: impl Future<Output = Result<R, AuthError>> + Send + 'static,
) -> Result<R, AuthError> {
    crate::rt().spawn(f).await.unwrap_or_else(|_| Err(AuthError { code: "unreachable".into(), status: 0 }))
}

#[cfg(target_arch = "wasm32")]
async fn asked<R>(f: impl Future<Output = Result<R, AuthError>>) -> Result<R, AuthError> {
    f.await
}

/// SignIn's own words where the client's would mislead here; else the client's.
fn said(e: &AuthError) -> String {
    match e.code.as_str() {
        "invalid_code" => "Wrong code — sign in again.".into(),
        "pending_expired" => "The code expired — sign in again.".into(),
        "invalid_or_expired_token" => "That reset token is used up or expired — ask for a new one.".into(),
        _ => e.to_string(),
    }
}

/// The form; `on_signed` gets the session (link with [`Client::link`]).
/// Parts: `signinId`, `signinEmail`, `signinPassword`, `signinCode`,
/// `signinToken`, `signinSubmit`, `signinToggle`, `signinForgot`,
/// `signinHaveToken`, `signinBack`, `signinError`, `signinNotice`.
#[component]
pub fn SignIn(
    client: Client,
    on_signed: EventHandler<Signed>,
    /// The heading (default: by what the form does).
    #[props(into)]
    title: Option<String>,
    /// Offer sign-up (when the server allows it).
    #[props(default = true)]
    signup: bool,
    /// Ask for an email on sign-up (verification and resets need one).
    #[props(default = true)]
    email: bool,
    /// Offer "Forgot password?" (when the server sends mail).
    #[props(default = true)]
    forgot: bool,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    let mode = signal(|| Mode::Login);
    let (id, password, code) = (signal(String::new), signal(String::new), signal(String::new));
    let (mail, token) = (signal(String::new), signal(String::new));
    let pending = signal(|| None::<String>);
    let error = signal(|| None::<String>);
    let notice = signal(|| None::<&'static str>);
    let busy = signal(|| false);
    let c = client.clone();
    let offers = resource(move || {
        let c = c.clone();
        async move { asked(async move { c.me(None).await }).await.ok() }
    });
    let me = offers.get().flatten().unwrap_or(Me::default());
    let submit = move |e: FormEvent| {
        e.prevent_default();
        if busy.get() {
            return;
        }
        let (c, m, p) = (client.clone(), mode.get(), pending.get());
        let (i, pw, cd, em, tk) = (id.get(), password.get(), code.get(), mail.get(), token.get());
        busy.set(true);
        error.set(None);
        spawn(async move {
            let got: Result<Done, AuthError> = asked(async move {
                let email = Some(em.as_str()).filter(|e| !e.is_empty());
                Ok(match (&p, m) {
                    (Some(p), _) => Done::Signed(c.totp(p, &cd).await?),
                    (None, Mode::Signup) => match c.signup(&i, &pw, email).await? {
                        Signup::Signed(s) => Done::Signed(s),
                        Signup::Verify { sent: true } => {
                            Done::Told("Account created — check your inbox to verify, then sign in.")
                        }
                        Signup::Verify { sent: false } => Done::Told(
                            "Account created, but the verification email could not be sent — try again later.",
                        ),
                    },
                    (None, Mode::Login) => match c.login(&i, &pw).await? {
                        Login::Signed(s) => Done::Signed(s),
                        Login::Totp { pending } => Done::Totp(pending),
                    },
                    (None, Mode::Forgot) => c.request_reset(&i).await.map(|_| Done::Asked)?,
                    (None, Mode::Reset) => {
                        c.reset(&tk, &pw).await.map(|_| Done::Told("Password changed — sign in with it now."))?
                    }
                })
            })
            .await;
            busy.set(false);
            // The person moved on (Back) while it was asked: only a session still counts.
            if *mode.read_untracked() != m && !matches!(got, Ok(Done::Signed(_))) {
                return;
            }
            code.set(String::new());
            match got {
                Ok(Done::Signed(s)) => {
                    pending.set(None);
                    password.set(String::new());
                    on_signed.call(s);
                }
                Ok(Done::Totp(p)) => pending.set(Some(p)),
                Ok(Done::Told(n)) => {
                    notice.set(Some(n));
                    password.set(String::new());
                    mode.set(Mode::Login);
                }
                Ok(Done::Asked) => {
                    notice.set(Some(
                        "If that account has an email on file, a reset token is on its way. Paste it below with \
                         your new password.",
                    ));
                    mode.set(Mode::Reset);
                }
                // The step is spent: its form would offer a retry that can't work.
                Err(e) if pending.read_untracked().is_some() => {
                    pending.set(None);
                    password.set(String::new());
                    error.set(Some(said(&e)));
                }
                Err(e) => error.set(Some(said(&e))),
            }
        });
    };
    let to = move |m: Mode| {
        mode.set(m);
        error.set(None);
        notice.set(None);
    };
    let (m, step) = (mode.get(), pending.get().is_some());
    let heading = title.clone().unwrap_or_else(|| {
        match (step, m) {
            (true, _) => "Two-factor code",
            (false, Mode::Login) => "Sign in",
            (false, Mode::Signup) => "Create account",
            (false, Mode::Forgot) => "Reset password",
            (false, Mode::Reset) => "Set a new password",
        }
        .into()
    });
    let action = match (step, m) {
        (true, _) => "Verify",
        (false, Mode::Login) => "Sign in",
        (false, Mode::Signup) => "Create account",
        (false, Mode::Forgot) => "Email me a reset token",
        (false, Mode::Reset) => "Set password",
    };
    let new = m == Mode::Signup;
    rsx! {
        form { class: join("aio-signin", &class), onsubmit: submit, ..attributes,
            h2 { class: "aio-signin__title", "{heading}" }
            if step {
                Field { label: "Code",
                    Input { value: code.get(), "name": "code", "inputmode": "numeric", "autocomplete": "one-time-code", placeholder: "6-digit code", "data-t": "signinCode", oninput: move |v| code.set(v) }
                }
            } else {
                if m != Mode::Reset {
                    Field { label: "User id",
                        Input { value: id.get(), "name": "id", "autocomplete": "username", "data-t": "signinId", oninput: move |v| id.set(v) }
                    }
                }
                if new && email {
                    Field { label: "Email",
                        Input { value: mail.get(), r#type: "email", "name": "email", "autocomplete": "email", "data-t": "signinEmail", oninput: move |v| mail.set(v) }
                    }
                }
                if m == Mode::Reset {
                    Field { label: "Reset token",
                        Input { value: token.get(), "name": "token", placeholder: "from the email", "data-t": "signinToken", oninput: move |v| token.set(v) }
                    }
                }
                if m != Mode::Forgot {
                    Field { label: if m == Mode::Reset { "New password" } else { "Password" },
                        Input { value: password.get(), r#type: "password", "name": "password", "autocomplete": if matches!(m, Mode::Login) { "current-password" } else { "new-password" }, "data-t": "signinPassword", oninput: move |v| password.set(v) }
                    }
                }
            }
            if let Some(e) = error.get() {
                Alert { tone: Tone::Error, "data-t": "signinError", "{e}" }
            }
            if let Some(n) = notice.get() {
                Alert { tone: Tone::Success, "data-t": "signinNotice", "{n}" }
            }
            Button { variant: Variant::Primary, r#type: "submit", disabled: busy.get(), "data-t": "signinSubmit", "{action}" }
            if !step {
                if signup && me.signup && matches!(m, Mode::Login | Mode::Signup) {
                    Button { variant: Variant::Ghost, "data-t": "signinToggle", onclick: move |_| to(if new { Mode::Login } else { Mode::Signup }),
                        if new { "Have an account? Sign in" } else { "New here? Create an account" }
                    }
                }
                if forgot && me.mail && m == Mode::Login {
                    Button { variant: Variant::Ghost, "data-t": "signinForgot", onclick: move |_| to(Mode::Forgot), "Forgot password?" }
                }
                if m == Mode::Forgot {
                    Button { variant: Variant::Ghost, "data-t": "signinHaveToken", onclick: move |_| to(Mode::Reset), "I already have a token" }
                }
                if matches!(m, Mode::Forgot | Mode::Reset) {
                    Button { variant: Variant::Ghost, "data-t": "signinBack", onclick: move |_| to(Mode::Login), "Back to sign in" }
                }
            }
        }
    }
}

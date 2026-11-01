use super::*;
use std::sync::atomic::{AtomicI64, Ordering};

const PW: &str = "correct horse battery";
const T0: i64 = 1_800_000_000_000;

fn auth(policy: Policy) -> (Auth, Arc<AtomicI64>) {
    let clock = Arc::new(AtomicI64::new(T0));
    (Auth::memory(policy).unwrap().on_clock(clock.clone()), clock)
}

fn signed(l: Login) -> String {
    match l {
        Login::Signed { token, .. } => token,
        Login::Totp { .. } => panic!("expected a session"),
    }
}

#[test]
fn sessions_issue_resolve_refresh_revoke() {
    let (a, clock) = auth(Policy { ttl_ms: 60_000, ..Default::default() });
    let t = a.issue("ann", "admin").unwrap();
    assert!(t.starts_with("aios_") && t.len() == 69);
    let s = a.session(&t).unwrap().unwrap();
    assert_eq!((s.user.as_str(), s.role.as_str(), s.expires_at), ("ann", "admin", T0 + 60_000));
    // Kept hashed: the raw token is nowhere in the file.
    let raw: i64 = lock(&a.db)
        .query_row("SELECT COUNT(*) FROM sessions WHERE token_hash = ?1 OR user_id = ?1", [&t], |r| r.get(0))
        .unwrap();
    assert_eq!(raw, 0);
    clock.fetch_add(10_000, Ordering::Relaxed);
    assert!(a.refresh(&t).unwrap());
    assert_eq!(a.session(&t).unwrap().unwrap().expires_at, T0 + 70_000);
    assert!(!a.refresh("aios_nope").unwrap());
    let ticks = a.changes();
    assert!(a.revoke(&t).unwrap());
    assert!(ticks.has_changed().unwrap(), "a revocation is told to open sockets");
    assert_eq!(a.session(&t).unwrap(), None);
    assert!(!a.revoke(&t).unwrap());
}

#[test]
fn sessions_expire_and_slide() {
    let (a, clock) = auth(Policy { ttl_ms: 60_000, ..Default::default() });
    let (used, idle) = (a.issue("ann", "user").unwrap(), a.issue("ann", "user").unwrap());
    // Used past half its life: renewed to a full TTL.
    clock.store(T0 + 40_000, Ordering::Relaxed);
    assert_eq!(a.session(&used).unwrap().unwrap().expires_at, T0 + 100_000);
    clock.store(T0 + 60_000, Ordering::Relaxed);
    assert_eq!(a.session(&idle).unwrap(), None, "expired at its TTL");
    assert!(a.session(&used).unwrap().is_some());
    assert_eq!(a.sessions().unwrap(), 1, "the expired row is gone");
    assert!(Auth::memory(Policy { ttl_ms: 0, ..Default::default() }).is_err());
    assert!(Auth::memory(Policy { ttl_ms: MAX_TTL_MS + 1, ..Default::default() }).is_err());
    assert!(Auth::memory(Policy { max_sessions: Some(0), ..Default::default() }).is_err());
}

#[test]
fn revoke_user_ends_only_that_users_sessions() {
    let (a, _) = auth(Policy::default());
    let (a1, a2, b) =
        (a.issue("ann", "user").unwrap(), a.issue("ann", "user").unwrap(), a.issue("bob", "user").unwrap());
    assert_eq!(a.revoke_user("ann").unwrap(), 2);
    assert_eq!((a.session(&a1).unwrap(), a.session(&a2).unwrap()), (None, None));
    assert!(a.session(&b).unwrap().is_some());
}

#[test]
fn max_sessions_keeps_the_newest_per_user() {
    let (a, _) = auth(Policy { max_sessions: Some(3), ..Default::default() });
    let ts: Vec<String> = (0..6).map(|_| a.issue("ann", "user").unwrap()).collect();
    let bob = a.issue("bob", "user").unwrap();
    let live: Vec<bool> = ts.iter().map(|t| a.session(t).unwrap().is_some()).collect();
    assert_eq!(live, [false, false, false, true, true, true]);
    assert!(a.session(&bob).unwrap().is_some(), "another user is untouched");
}

#[test]
fn a_lowered_cap_holds_from_the_open() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("auth.db");
    let ts: Vec<String> = {
        let a = Auth::open(&p, Policy::default()).unwrap();
        (0..4).map(|_| a.issue("ann", "user").unwrap()).collect()
    };
    let a = Auth::open(&p, Policy { max_sessions: Some(2), ..Default::default() }).unwrap();
    let live: Vec<bool> = ts.iter().map(|t| a.session(t).unwrap().is_some()).collect();
    assert_eq!(live, [false, false, true, true]);
}

#[test]
fn the_role_is_read_live() {
    let (a, _) = auth(Policy::default());
    a.create("ann", PW, Some("admin")).unwrap();
    let t = signed(a.login("ann", PW).unwrap());
    assert_eq!(a.session(&t).unwrap().unwrap().role, "admin");
    let ticks = a.changes();
    assert!(a.set_role("ann", "user").unwrap());
    assert!(ticks.has_changed().unwrap());
    assert_eq!(a.session(&t).unwrap().unwrap().role, "user", "a demotion counts at once");
}

#[test]
fn ids_are_one_key_however_typed() {
    let (a, _) = auth(Policy::default());
    a.create("neighbour", PW, None).unwrap();
    for dup in ["Neighbour", "NEIGHBOUR", "  neighbour  "] {
        assert_eq!(a.create(dup, PW, None), Err(Refusal::UserExists), "{dup}");
    }
    // NFD spelling of "sué" is the NFC one; Unicode case folds too.
    a.create("su\u{e9}", PW, None).unwrap();
    assert_eq!(a.create("sue\u{301}", PW, None), Err(Refusal::UserExists));
    assert_eq!(a.create("SU\u{c9}", PW, None), Err(Refusal::UserExists));
    // NFKC: full width, ligatures and letterlike forms are the same id.
    a.create("admin", PW, None).unwrap();
    for twin in ["\u{ff41}\u{ff44}\u{ff4d}\u{ff49}\u{ff4e}", "\u{ff21}dmin", "ADMIN"] {
        assert_eq!(a.create(twin, PW, None), Err(Refusal::UserExists), "{twin}");
    }
    a.create("\u{fb01}le", PW, None).unwrap();
    assert_eq!(a.create("file", PW, None), Err(Refusal::UserExists), "the ligature fi");
    assert_eq!(a.create("\u{ff2f}IDC:x", PW, None), Err(Refusal::ReservedId), "a full-width O");
    for bad in ["   ", "", "a\u{200d}b", "a\0b", "john doe", "a\u{202e}b", &"x".repeat(257)] {
        assert_eq!(a.create(bad, PW, None), Err(Refusal::InvalidId), "{bad:?}");
    }
    assert_eq!(a.create("OIDC:issuer:sub", PW, None), Err(Refusal::ReservedId));
    assert_eq!(a.create("ann", "short", None), Err(Refusal::PasswordTooShort));
    assert_eq!(a.users().unwrap().len(), 4);
    assert_eq!(a.user(" neighbour ").unwrap().unwrap().id, "neighbour");
    assert_eq!(a.verify("sue\u{301}", PW).unwrap().id, "su\u{e9}");
}

#[test]
fn lockout_after_five_says_locked_only_to_the_right_password() {
    let (a, clock) = auth(Policy::default());
    a.create("ann", PW, None).unwrap();
    for _ in 0..4 {
        assert_eq!(a.verify("ann", "wrong password"), Err(Refusal::InvalidCredentials));
    }
    // A right one before the fifth resets the count.
    a.verify("ann", PW).unwrap();
    for _ in 0..5 {
        assert_eq!(a.verify("ann", "wrong password"), Err(Refusal::InvalidCredentials));
    }
    assert_eq!(a.verify("ann", "wrong password"), Err(Refusal::InvalidCredentials), "no oracle for guessers");
    assert_eq!(a.verify("ann", PW), Err(Refusal::Locked));
    clock.fetch_add(LOCK_MS, Ordering::Relaxed);
    assert_eq!(a.verify("ann", PW).unwrap().id, "ann");
    // Unknown ids are refused the same way, and lock nothing.
    assert_eq!(a.verify("nobody", PW), Err(Refusal::InvalidCredentials));
}

#[test]
fn a_burst_of_guesses_counts_as_the_same_guesses_one_by_one() {
    let (a, _) = auth(Policy::default());
    a.create("ann", PW, None).unwrap();
    let a = Arc::new(a);
    let tries: Vec<_> = (0..8)
        .map(|_| {
            let a = a.clone();
            std::thread::spawn(move || a.verify("ann", "wrong password"))
        })
        .collect();
    for t in tries {
        assert_eq!(t.join().unwrap(), Err(Refusal::InvalidCredentials));
    }
    assert_eq!(a.verify("ann", PW), Err(Refusal::Locked));
    assert!(a.unlock("ann").unwrap());
    a.verify("ann", PW).unwrap();
    assert!(lock(&a.lanes).is_empty(), "no lane left behind");
}

#[test]
fn a_new_password_rescues_the_account_from_everyone() {
    let (a, _) = auth(Policy::default());
    a.create("ann", PW, None).unwrap();
    let t = signed(a.login("ann", PW).unwrap());
    let pending = a.one_shot("totp", "ann", TOTP_PENDING_MS).unwrap();
    for _ in 0..5 {
        _ = a.verify("ann", "wrong password");
    }
    assert!(a.set_password("ann", "a new long password").unwrap());
    assert_eq!(a.session(&t).unwrap(), None, "every session revoked");
    assert_eq!(a.consume("totp", &pending).unwrap(), None, "every pending token burned");
    a.verify("ann", "a new long password").unwrap();
    assert_eq!(a.verify("ann", PW), Err(Refusal::InvalidCredentials));
    assert_eq!(a.set_password("ann", "short"), Err(Refusal::PasswordTooShort));
    assert!(!a.set_password("nobody", PW).unwrap());
}

#[test]
fn removing_a_user_ends_their_sessions() {
    let (a, _) = auth(Policy::default());
    a.create("ann", PW, None).unwrap();
    let t = signed(a.login("ann", PW).unwrap());
    assert!(a.remove("ann").unwrap());
    assert_eq!(a.session(&t).unwrap(), None);
    assert!(a.user("ann").unwrap().is_none());
    assert!(!a.remove("ann").unwrap());
}

#[test]
fn a_new_email_is_unverified() {
    let (a, _) = auth(Policy::default());
    a.create("ann", PW, None).unwrap();
    a.set_email("ann", "a@example.org").unwrap();
    a.mark_verified("ann").unwrap();
    a.set_email("ann", "a@example.org").unwrap();
    assert!(a.user("ann").unwrap().unwrap().verified, "the same address stays verified");
    a.set_email("ann", "b@example.org").unwrap();
    let u = a.user("ann").unwrap().unwrap();
    assert_eq!((u.email.as_deref(), u.verified), (Some("b@example.org"), false));
}

fn code_at(a: &Auth, id: &str, at: i64) -> String {
    let secret: String = lock(&a.db).query_row("SELECT totp FROM users WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    totp::code(&totp::unbase32(&secret).unwrap(), at / totp::STEP_MS)
}

fn with_totp(a: &Auth, clock: &AtomicI64) {
    a.create("ann", PW, None).unwrap();
    assert_eq!(a.totp_enable("ann", "000000"), Err(Refusal::SetupFirst));
    a.totp_setup("ann").unwrap();
    let now = clock.load(Ordering::Relaxed);
    assert_eq!(a.totp_enable("ann", "000000"), Err(Refusal::InvalidCode));
    a.totp_enable("ann", &code_at(a, "ann", now - totp::STEP_MS)).unwrap();
    assert_eq!(a.totp_setup("ann"), Err(Refusal::TotpOn));
    assert!(a.user("ann").unwrap().unwrap().totp);
}

#[test]
fn totp_logins_take_two_steps_and_a_code_works_once() {
    let (a, clock) = auth(Policy::default());
    with_totp(&a, &clock);
    let Login::Totp { pending } = a.login("ann", PW).unwrap() else { panic!("a second step is due") };
    let code = code_at(&a, "ann", T0);
    let (t, u) = a.login_totp(&pending, &code).unwrap();
    assert_eq!((u.id.as_str(), a.session(&t).unwrap().is_some()), ("ann", true));
    // The pending token is spent, and so is the code.
    assert_eq!(a.login_totp(&pending, &code), Err(Refusal::PendingExpired));
    let Login::Totp { pending } = a.login("ann", PW).unwrap() else { panic!() };
    assert_eq!(a.login_totp(&pending, &code), Err(Refusal::InvalidCode));
    // A pending token expires.
    let Login::Totp { pending } = a.login("ann", PW).unwrap() else { panic!() };
    clock.fetch_add(TOTP_PENDING_MS, Ordering::Relaxed);
    assert_eq!(a.login_totp(&pending, "123456"), Err(Refusal::PendingExpired));
}

#[test]
fn a_spent_code_stays_spent_across_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("auth.db");
    let clock = Arc::new(AtomicI64::new(T0));
    let code = {
        let a = Auth::open(&p, Policy::default()).unwrap().on_clock(clock.clone());
        with_totp(&a, &clock);
        let Login::Totp { pending } = a.login("ann", PW).unwrap() else { panic!() };
        let code = code_at(&a, "ann", T0);
        a.login_totp(&pending, &code).unwrap();
        code
    };
    let a = Auth::open(&p, Policy::default()).unwrap().on_clock(clock.clone());
    let Login::Totp { pending } = a.login("ann", PW).unwrap() else { panic!() };
    assert_eq!(a.login_totp(&pending, &code), Err(Refusal::InvalidCode));
    // The next step's code still works.
    clock.fetch_add(totp::STEP_MS, Ordering::Relaxed);
    let Login::Totp { pending } = a.login("ann", PW).unwrap() else { panic!() };
    a.login_totp(&pending, &code_at(&a, "ann", T0 + totp::STEP_MS)).unwrap();
}

#[test]
fn wrong_codes_lock_the_account_and_burn_pending_steps() {
    let (a, clock) = auth(Policy::default());
    with_totp(&a, &clock);
    // Many second steps opened at once (from rotating addresses)…
    let pendings: Vec<String> = (0..8)
        .map(|_| match a.login("ann", PW).unwrap() {
            Login::Totp { pending } => pending,
            Login::Signed { .. } => panic!(),
        })
        .collect();
    let answers: Vec<Refusal> = pendings[..5].iter().map(|p| a.login_totp(p, "000000").unwrap_err()).collect();
    assert_eq!(answers[..4], [Refusal::InvalidCode, Refusal::InvalidCode, Refusal::InvalidCode, Refusal::InvalidCode]);
    assert_eq!(answers[4], Refusal::Locked);
    // …can't keep guessing once it locks: the rest were burned.
    let right = code_at(&a, "ann", T0);
    assert_eq!(a.login_totp(&pendings[5], &right), Err(Refusal::PendingExpired));
    assert_eq!(a.login("ann", PW), Err(Refusal::Locked));
    clock.fetch_add(LOCK_MS, Ordering::Relaxed);
    let Login::Totp { pending } = a.login("ann", PW).unwrap() else { panic!() };
    a.login_totp(&pending, &code_at(&a, "ann", T0 + LOCK_MS)).unwrap();
}

#[test]
fn the_password_alone_does_not_reset_the_code_count() {
    let (a, clock) = auth(Policy::default());
    with_totp(&a, &clock);
    for _ in 0..4 {
        let Login::Totp { pending } = a.login("ann", PW).unwrap() else { panic!() };
        assert_eq!(a.login_totp(&pending, "000000"), Err(Refusal::InvalidCode));
    }
    let Login::Totp { pending } = a.login("ann", PW).unwrap() else { panic!() };
    assert_eq!(a.login_totp(&pending, "000000"), Err(Refusal::Locked));
}

#[test]
fn totp_off_clears_the_factor() {
    let (a, clock) = auth(Policy::default());
    with_totp(&a, &clock);
    assert!(a.totp_off("ann").unwrap());
    assert!(!a.totp_off("ann").unwrap());
    signed(a.login("ann", PW).unwrap());
    // A re-enrolment starts its own count of used steps.
    a.totp_setup("ann").unwrap();
    a.totp_enable("ann", &code_at(&a, "ann", T0 - totp::STEP_MS)).unwrap();
}

#[test]
fn aios_auth_db_opens_as_is() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("auth.db");
    {
        // aio's tables, before it added `totp_step`, with an aio hash of "passwd".
        let c = Connection::open(&p).unwrap();
        c.execute_batch(SCHEMA).unwrap();
        c.execute(
            "INSERT INTO users (id, pw, role, created_at) VALUES ('ann', 'pbkdf2$sha256$1$73616c74$55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc', 'admin', 1)",
            [],
        )
        .unwrap();
    }
    let a = Auth::open(&p, Policy::default()).unwrap();
    assert_eq!(a.verify("ann", "passwd").unwrap().role, "admin");
}

#[test]
fn a_made_password_draws_each_letter_uniformly() {
    // aio am-auth: `byte % 62` alone would favour the first eight by 25%.
    let n = 4000;
    let mut counts = std::collections::HashMap::new();
    for _ in 0..n {
        let pw = generated_password();
        assert_eq!(pw.len(), 16);
        for c in pw.chars() {
            *counts.entry(c).or_insert(0usize) += 1;
        }
    }
    assert_eq!(counts.len(), 62, "every letter and digit comes up");
    // 5σ of a Bernoulli(1/62) count over n·16 draws: false alarms ~1 in 57,000 runs.
    let expected = (n * 16) as f64 / 62.0;
    let sigma = (expected * (1.0 - 1.0 / 62.0)).sqrt();
    assert!(counts.values().all(|c| (*c as f64 - expected).abs() < 5.0 * sigma), "{counts:?}");
}

/// Every scalar value alone, after a base letter, with marks to reorder
/// (ccc 220 before 230, in both orders), as conjoining Hangul jamo, and
/// each followed by a mark that may compose with it: ids typed any way.
fn nfc_corpus() -> impl Iterator<Item = String> {
    let all = || (0..=0x10FFFFu32).filter_map(char::from_u32);
    let alone = all().map(String::from);
    let marked = all().map(|c| format!("{c}\u{301}"));
    let reordered = all().flat_map(|c| [format!("a{c}\u{323}\u{302}"), format!("{c}\u{302}\u{323}")]);
    let jamo = (0x1100..0x1113u32).flat_map(|l| {
        (0x1161..0x1176u32).flat_map(move |v| (0x11A7..0x11C3u32).map(move |t| [l, v, t]))
    });
    let jamo = jamo.map(|j| j.iter().filter_map(|&c| char::from_u32(c)).collect::<String>());
    alone.chain(marked).chain(reordered).chain(jamo)
}

fn nfc_digest(f: impl Fn(&str) -> String) -> String {
    let mut h = ring::digest::Context::new(&ring::digest::SHA256);
    for s in nfc_corpus() {
        h.update(f(&s).as_bytes());
        h.update(&[0]);
    }
    hex(h.finish().as_ref())
}

/// What ICU4X (icu_normalizer 2.4, the NFC ids were keyed by before)
/// made of the corpus: the same keys, so the same state.db both ways.
const ICU_NFC_DIGEST: &str = "908500fa84ebdfbdf28c210e80afff8da8161f06594f7c4d5531d5c8a0047652";

#[test]
fn ids_normalize_as_icu_did_over_the_corpus() {
    use unicode_normalization::UnicodeNormalization;
    assert_eq!(nfc_digest(|s| s.nfc().collect()), ICU_NFC_DIGEST);
    assert_eq!(normalize(" Ame\u{301}lie\t"), "Am\u{e9}lie");
}

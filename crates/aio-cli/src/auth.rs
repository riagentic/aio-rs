//! `ax auth <app> …` (aio: `am auth`): the operator console for the app's
//! accounts, straight on its `auth.db`, running or not: it seeds the first
//! admin before the app ever booted, and it is the way back in when locked
//! out. Every change reaches the running app's next request (sessions and
//! roles are read from the database each time).
use aio::auth::{Auth, Policy, User};
use std::path::Path;

type Out = Result<String, String>;

/// What `ax auth` can do, for the usage line and its refusals.
pub const DOES: &str = "users · create <id> · passwd <id> · role <id> <role> · totp <id> off · unlock <id> · verify <id> · revoke <id> · rm <id>";

/// The fields `create` and `passwd` take.
pub struct Fields<'a> {
    pub role: Option<&'a str>,
    pub email: Option<&'a str>,
    pub password: Option<&'a str>,
}

fn row(u: &User, locked: bool) -> String {
    let date = aio::log::stamp(u.created_at);
    format!(
        "{:<24} {:<8} {:<28} {}{}{}  {}",
        u.id,
        u.role,
        u.email.as_deref().unwrap_or("-"),
        if u.verified { "verified" } else { "unverified" },
        if u.totp { " 2FA" } else { "" },
        if locked { " locked" } else { "" },
        &date[..10]
    )
}

/// The ids locked out now.
fn locked(file: &Path, now: i64) -> Result<Vec<String>, String> {
    let c = rusqlite::Connection::open_with_flags(file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let mut q = c.prepare("SELECT id FROM users WHERE locked_until > ?1").map_err(|e| e.to_string())?;
    let ids = q.query_map([now], |r| r.get(0)).map_err(|e| e.to_string())?;
    ids.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

/// Run `what` (`[do, id, value]`) on the accounts in `data/auth.db`.
pub fn run(data: &Path, what: &[&str], f: &Fields, json: bool, now: i64) -> Out {
    let file = data.join("auth.db");
    let (sub, id, value) = (what[0], what.get(1).copied(), what.get(2).copied());
    // Only these take an id, a value, fields.
    let (ids, values, fields): (bool, bool, &[&str]) = match sub {
        "users" => (false, false, &[]),
        "create" => (true, false, &["role", "email", "password"]),
        "passwd" => (true, false, &["password"]),
        "role" | "totp" => (true, true, &[]),
        "unlock" | "verify" | "revoke" | "rm" => (true, false, &[]),
        _ => return Err(format!("no `ax auth {sub}`: {DOES}")),
    };
    let given = [("role", f.role), ("email", f.email), ("password", f.password)];
    if let Some((name, _)) = given.iter().find(|(n, v)| v.is_some() && !fields.contains(n)) {
        return Err(format!("ax auth {sub} takes no --{name}"));
    }
    match (ids, id, values, value) {
        (true, None, ..) => return Err(format!("ax auth {sub} <id>{}", if values { " <value>" } else { "" })),
        (false, Some(w), ..) | (_, _, false, Some(w)) => return Err(format!("ax auth {sub}: one word too many: {w}")),
        (_, _, true, None) => {
            return Err(format!("ax auth {sub} <id> <{}>", if sub == "role" { "role" } else { "off" }));
        }
        _ => {}
    }
    if !file.exists() && sub != "create" {
        return Err(format!(
            "no auth.db at {}: no accounts yet (ax auth <app> create <id> makes the first)",
            file.display()
        ));
    }
    if !data.is_dir() {
        return Err(format!("no data at {}: has the app run here? (ax data <app>)", data.display()));
    }
    let a = Auth::open(&file, Policy::default()).map_err(|e| e.to_string())?;
    let id = id.unwrap_or("");
    let no_such = || format!("no user `{id}`");
    let say = |obj: serde_json::Value, text: String| if json { obj.to_string() } else { text };
    match sub {
        "users" => {
            let users = a.users().map_err(|e| e.to_string())?;
            let out = locked(&file, now)?;
            if json {
                let rows = users.iter().map(|u| {
                    serde_json::json!({ "id": u.id, "role": u.role, "email": u.email, "verified": u.verified,
                        "totp": u.totp, "locked": out.contains(&u.id), "created": u.created_at })
                });
                return Ok(serde_json::Value::Array(rows.collect()).to_string());
            }
            Ok(if users.is_empty() {
                "no users yet".into()
            } else {
                users.iter().map(|u| row(u, out.contains(&u.id))).collect::<Vec<_>>().join("\n")
            })
        }
        // An identity provider's account: it signs in there, never with a password.
        "create" if id.starts_with(aio::auth::EXTERNAL_PREFIX) => {
            let u = a
                .create_external(id, f.role.unwrap_or("user"), None)
                .map_err(|e| format!("can't create `{id}`: {e}"))?;
            if let Some(e) = f.email {
                a.set_email(&u.id, e).map_err(|e| e.to_string())?;
            }
            let text = format!("created {} ({}): it signs in with its identity provider", u.id, u.role);
            Ok(say(serde_json::json!({ "id": u.id, "role": u.role, "email": f.email }), text))
        }
        "create" => {
            let password = f.password.map_or_else(aio::auth::generated_password, String::from);
            let u = a.create(id, &password, f.role).map_err(|e| format!("can't create `{id}`: {e}"))?;
            if let Some(e) = f.email {
                a.set_email(&u.id, e).map_err(|e| e.to_string())?;
            }
            let shown = f.password.is_none().then_some(password);
            let text = format!(
                "created {} ({}){}",
                u.id,
                u.role,
                shown.as_ref().map_or(String::new(), |p| format!(": password {p}"))
            );
            Ok(say(serde_json::json!({ "id": u.id, "role": u.role, "email": f.email, "password": shown }), text))
        }
        "passwd" => {
            let password = f.password.map_or_else(aio::auth::generated_password, String::from);
            if !a.set_password(id, &password).map_err(|e| e.to_string())? {
                return Err(no_such());
            }
            let shown = f.password.is_none().then_some(password);
            let text = format!(
                "password set for {id} (lockout cleared, sessions ended){}",
                shown.as_ref().map_or(String::new(), |p| format!(": password {p}"))
            );
            Ok(say(serde_json::json!({ "id": id, "password": shown, "sessionsEnded": true }), text))
        }
        "role" => {
            let role = value.unwrap_or("");
            if !a.set_role(id, role).map_err(|e| e.to_string())? {
                return Err(no_such());
            }
            Ok(say(
                serde_json::json!({ "id": id, "role": role }),
                format!("{id} has role {role} now, in its live sessions too"),
            ))
        }
        "totp" => {
            if value != Some("off") {
                return Err("ax auth totp <id> off (there is no on: a user enrols their own second factor)".into());
            }
            if a.user(id).map_err(|e| e.to_string())?.is_none() {
                return Err(no_such());
            }
            let had = a.totp_off(id).map_err(|e| e.to_string())?;
            let text = format!("{id}: second factor cleared{}", if had { "" } else { " (none was set)" });
            Ok(say(serde_json::json!({ "id": id, "totp": false, "cleared": had }), text))
        }
        "unlock" | "verify" | "rm" => {
            let done = match sub {
                "unlock" => a.unlock(id),
                "verify" => a.mark_verified(id),
                _ => a.remove(id),
            };
            if !done.map_err(|e| e.to_string())? {
                return Err(no_such());
            }
            let what = match sub {
                "unlock" => "unlocked",
                "verify" => "marked verified",
                _ => "removed (sessions ended)",
            };
            Ok(say(serde_json::json!({ "id": id, sub: true }), format!("{id} {what}")))
        }
        _ => {
            if a.user(id).map_err(|e| e.to_string())?.is_none() {
                return Err(no_such());
            }
            let n = a.end_all(id).map_err(|e| e.to_string())?;
            Ok(say(
                serde_json::json!({ "id": id, "sessionsEnded": n }),
                format!("ended {n} session(s) and every pending token of {id}"),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Fields = Fields { role: None, email: None, password: None };

    fn now() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
    }

    fn auth(data: &Path, line: &str, f: &Fields) -> Out {
        run(data, &line.split(' ').collect::<Vec<_>>(), f, false, now())
    }

    #[test]
    fn the_operator_seeds_rescues_and_removes_accounts() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path();
        assert!(auth(data, "users", &NONE).unwrap_err().contains("no accounts yet"));
        // The first admin, before the app ever ran; a password made to hand on.
        let made =
            auth(data, "create ana", &Fields { role: Some("admin"), email: Some("ana@example.com"), password: None });
        let made = made.unwrap();
        let pw = made.rsplit_once("password ").unwrap().1.to_string();
        assert!(made.starts_with("created ana (admin): password ") && pw.len() == 16, "{made}");
        assert!(pw.chars().all(|c| c.is_ascii_alphanumeric()));
        let a = Auth::open(data.join("auth.db"), Policy::default()).unwrap();
        a.verify("ana", &pw).unwrap();
        let token = a.issue("ana", "admin").unwrap();
        // A password given is not echoed; one too short is refused.
        let bo = Fields { password: Some("correct horse"), ..NONE };
        assert_eq!(auth(data, "create bo", &bo), Ok("created bo (user)".into()));
        assert!(
            auth(data, "create cy", &Fields { password: Some("short"), ..NONE }).unwrap_err().contains("can't create")
        );
        assert!(auth(data, "create bo", &bo).unwrap_err().contains("can't create `bo`"));
        // An identity provider's account is seeded without a password.
        let sso = Fields { role: Some("admin"), ..NONE };
        let made = auth(data, "create oidc:idp.example:Ana", &sso).unwrap();
        assert_eq!(made, "created oidc:idp.example:Ana (admin): it signs in with its identity provider");
        assert!(auth(data, "create oidc:idp.example:Ana", &sso).unwrap_err().contains("can't create"));
        a.remove("oidc:idp.example:Ana").unwrap();
        let list = auth(data, "users", &NONE).unwrap();
        assert!(list.lines().any(|l| l.starts_with("ana ") && l.contains("ana@example.com")), "{list}");
        // A role change holds in a live session at once.
        assert!(auth(data, "role ana user", &NONE).unwrap().contains("in its live sessions too"));
        assert_eq!(a.session(&token).unwrap().unwrap().role, "user");
        // A new password ends every session.
        assert!(auth(data, "passwd ana", &NONE).unwrap().contains("sessions ended"));
        assert!(a.session(&token).unwrap().is_none());
        let t2 = a.issue("bo", "user").unwrap();
        assert!(auth(data, "revoke bo", &NONE).unwrap().starts_with("ended 1 session(s)"));
        assert!(a.session(&t2).unwrap().is_none());
        assert_eq!(auth(data, "verify bo", &NONE), Ok("bo marked verified".into()));
        assert_eq!(auth(data, "unlock bo", &NONE), Ok("bo unlocked".into()));
        assert!(auth(data, "totp bo off", &NONE).unwrap().contains("(none was set)"));
        assert_eq!(auth(data, "rm bo", &NONE), Ok("bo removed (sessions ended)".into()));
        assert_eq!(auth(data, "rm bo", &NONE), Err("no user `bo`".into()));
        let rows: serde_json::Value =
            serde_json::from_str(&run(data, &["users"], &NONE, true, now()).unwrap()).unwrap();
        assert_eq!((rows.as_array().map(Vec::len), rows[0]["role"].as_str()), (Some(1), Some("user")));
    }

    #[test]
    fn a_lockout_shows_and_words_are_checked() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path();
        auth(data, "create ana", &Fields { password: Some("correct horse"), ..NONE }).unwrap();
        let a = Auth::open(data.join("auth.db"), Policy::default()).unwrap();
        for _ in 0..aio::auth::LOCK_AFTER {
            _ = a.verify("ana", "wrong password");
        }
        assert!(auth(data, "users", &NONE).unwrap().contains(" locked"), "locked out after many misses");
        auth(data, "unlock ana", &NONE).unwrap();
        assert!(!auth(data, "users", &NONE).unwrap().contains(" locked"));
        assert!(auth(data, "frob", &NONE).unwrap_err().contains("no `ax auth frob`"));
        assert_eq!(auth(data, "role ana", &NONE), Err("ax auth role <id> <role>".into()));
        assert!(auth(data, "totp ana on", &NONE).unwrap_err().contains("there is no on"));
        assert!(auth(data, "unlock", &NONE).unwrap_err().contains("ax auth unlock <id>"));
        assert!(auth(data, "users ana", &NONE).unwrap_err().contains("one word too many: ana"));
        assert!(auth(data, "rm ana", &Fields { role: Some("x"), ..NONE }).unwrap_err().contains("takes no --role"));
    }
}

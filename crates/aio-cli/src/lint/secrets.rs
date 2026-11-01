//! Credentials: field names that spell one, values that look like one, and
//! the well-known key formats, in Rust tokens and in every other text file.
use super::{Issue, Severity};
use proc_macro2::{Spacing, TokenStream, TokenTree};

/// A name split into its words: `apiKey` → api key, `PRIVATE_KEY` → private
/// key, `seed2` → seed 2. Words, never substrings: `monkey` is no `key`.
pub fn words(name: &str) -> Vec<String> {
    let c: Vec<char> = name.chars().collect();
    let (mut out, mut cur) = (Vec::new(), String::new());
    for (i, &ch) in c.iter().enumerate() {
        if !ch.is_ascii_alphanumeric() {
            out.extend((!cur.is_empty()).then(|| std::mem::take(&mut cur)));
            continue;
        }
        if let Some(&p) = c.get(i.wrapping_sub(1)).filter(|_| !cur.is_empty()) {
            let hump = (p.is_ascii_lowercase() || p.is_ascii_digit()) && ch.is_ascii_uppercase();
            let acronym = p.is_ascii_uppercase()
                && ch.is_ascii_uppercase()
                && c.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            if hump || acronym || p.is_ascii_alphabetic() != ch.is_ascii_alphabetic() {
                out.push(std::mem::take(&mut cur));
            }
        }
        cur.push(ch.to_ascii_lowercase());
    }
    out.extend((!cur.is_empty()).then_some(cur));
    out
}

/// Last words that make a name metadata about a secret, not the secret.
const META: [&str; 44] = [
    "id", "ids", "type", "name", "count", "index", "idx", "at", "ref", "kind", "length", "len", "path", "mode",
    "status", "flag", "enabled", "visible", "label", "order", "version", "ms", "sec", "secs", "seconds", "bytes", "kb",
    "mb", "gb", "hz", "pct", "percent", "ratio", "rate", "total", "avg", "min", "max", "size", "width", "height",
    "duration", "elapsed", "url",
];

/// `public_key`, `pubKey`, `key_name`: shared on purpose, or about a secret.
fn exempt(w: &[String]) -> bool {
    w.iter().any(|x| x == "pub" || x == "public") || (w.len() > 1 && w.last().is_some_and(|x| META.contains(&&**x)))
}

fn pair(w: &[String], a: &str, bs: &[&str]) -> bool {
    w.windows(2).any(|p| p[0] == a && bs.contains(&&*p[1]))
}

/// A name that unambiguously holds a credential (aio's boot refusal list):
/// password, passphrase, mnemonic, or private/api/secret key, access/auth token.
pub fn credential(name: &str) -> bool {
    let w = words(name);
    let hard = w.iter().any(|x| ["password", "passwrd", "passphrase", "mnemonic"].contains(&&**x))
        || pair(&w, "private", &["key"])
        || pair(&w, "api", &["key"])
        || pair(&w, "secret", &["key"])
        || pair(&w, "access", &["token"])
        || pair(&w, "auth", &["token"]);
    hard && !exempt(&w)
}

/// What a value assigned to `name` would be, and how long it must be to count.
fn rule(name: &str) -> Option<(&'static str, usize)> {
    let w = words(name);
    if exempt(&w) {
        return None;
    }
    let has = |x: &str| w.iter().any(|y| y == x);
    if ["api", "private", "secret", "access"].iter().any(|a| pair(&w, a, &["key"])) {
        Some(("key", 10))
    } else if has("password") || has("passwd") || has("passwrd") || has("passphrase") {
        Some(("password", 4))
    } else if has("token") {
        Some(("token", 20))
    } else if has("secret") {
        Some(("secret", 8))
    } else {
        None
    }
}

/// Does `value`, assigned to `name`, look like a real hardcoded secret?
pub fn hardcoded(name: &str, value: &str) -> Option<&'static str> {
    let (what, min) = rule(name)?;
    let low = value.to_lowercase();
    let placeholder = ["example", "changeme", "placeholder", "redacted", "xxxx", "****", "…"];
    let token_chars = value.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    // A key or secret is one run of characters: a value with spaces or
    // calls is code or prose (`const HAS_SECRET: &str = "<a script>"`).
    let code = value.contains(char::is_whitespace) || value.contains(['(', ')', ';']);
    (value.chars().count() >= min
        && (what == "password" || !code)
        && !value.contains(['{', '$', '<', '%'])
        && !placeholder.iter().any(|p| low.contains(p))
        && (what != "token" || token_chars))
        .then_some(what)
}

/// Prefix, the least run after it, which characters run, what it is.
type Format = (&'static str, usize, fn(char) -> bool, &'static str);

const FORMATS: [Format; 13] = [
    ("AKIA", 16, |c| c.is_ascii_uppercase() || c.is_ascii_digit(), "an AWS access key"),
    ("ghp_", 36, |c| c.is_ascii_alphanumeric(), "a GitHub token"),
    ("gho_", 36, |c| c.is_ascii_alphanumeric(), "a GitHub token"),
    ("ghu_", 36, |c| c.is_ascii_alphanumeric(), "a GitHub token"),
    ("ghs_", 36, |c| c.is_ascii_alphanumeric(), "a GitHub token"),
    ("github_pat_", 22, |c| c.is_ascii_alphanumeric() || c == '_', "a GitHub token"),
    ("xoxb-", 10, |c| c.is_ascii_alphanumeric() || c == '-', "a Slack token"),
    ("xoxp-", 10, |c| c.is_ascii_alphanumeric() || c == '-', "a Slack token"),
    ("AIza", 35, |c| c.is_ascii_alphanumeric() || "-_".contains(c), "a Google API key"),
    ("sk_live_", 16, |c| c.is_ascii_alphanumeric(), "a Stripe live key"),
    ("rk_live_", 16, |c| c.is_ascii_alphanumeric(), "a Stripe live key"),
    ("sk-", 20, |c| c.is_ascii_alphanumeric() || "-_".contains(c), "an API secret key"),
    ("glpat-", 20, |c| c.is_ascii_alphanumeric() || "-_".contains(c), "a GitLab token"),
];

/// The well-known key format `text` holds, if any.
pub fn known_key(text: &str) -> Option<&'static str> {
    if text.contains("-----BEGIN") && text.contains("PRIVATE KEY-----") {
        return Some("a private key");
    }
    FORMATS.iter().find_map(|&(prefix, min, ok, what)| {
        text.match_indices(prefix).find_map(|(at, _)| {
            let free = !text[..at].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
            let run: String = text[at + prefix.len()..].chars().take_while(|&c| ok(c)).collect();
            // `sk-` also starts ordinary words: a key has digits in it.
            let keyish = prefix != "sk-" || run.chars().any(|c| c.is_ascii_digit());
            (free && run.len() >= min && keyish).then_some(what)
        })
    })
}

fn found(file: &str, line: usize, name: &str, what: &str) -> Issue {
    Issue::new(Severity::Warn, "security", format!("possible hardcoded {what} in `{name}`"), file, line)
        .fix("read it at run time (the environment, the OS keychain), never from the source")
}

fn leaked(file: &str, line: usize, what: &str) -> Issue {
    Issue::new(Severity::Error, "security", format!("{what} is in the source"), file, line)
        .fix("revoke it, then load the new one at run time")
}

fn string(t: &TokenTree) -> Option<String> {
    syn::parse2::<syn::LitStr>(t.clone().into()).ok().map(|l| l.value())
}

fn alone(t: Option<&TokenTree>, ch: char) -> bool {
    matches!(t, Some(TokenTree::Punct(p)) if p.as_char() == ch && p.spacing() == Spacing::Alone)
}

/// Rust code (comments are gone; doc comments are string literals): every
/// string that is a known key, and `name = "…"` / `name: "…"` assignments.
pub fn tokens(file: &str, ts: TokenStream, out: &mut Vec<Issue>) {
    let tt: Vec<TokenTree> = ts.into_iter().collect();
    for (i, t) in tt.iter().enumerate() {
        if let TokenTree::Group(g) = t {
            tokens(file, g.stream(), out);
            continue;
        }
        let line = t.span().start().line;
        if let Some(what) = string(t).as_deref().and_then(known_key) {
            out.push(leaked(file, line, what));
            continue;
        }
        let name = match t {
            TokenTree::Ident(id) => id.to_string(),
            TokenTree::Literal(_) => match string(t) {
                Some(s) => s,
                None => continue,
            },
            _ => continue,
        };
        if !(alone(tt.get(i + 1), '=') || alone(tt.get(i + 1), ':')) {
            continue;
        }
        // The value, past a type (`const KEY: &str = "…"`), within the item.
        let value = tt[i + 2..]
            .iter()
            .take(8)
            .take_while(|t| !matches!(t, TokenTree::Group(_)) && !alone(Some(t), ';') && !alone(Some(t), ','))
            .find_map(string);
        if let Some(what) = value.and_then(|v| hardcoded(&name, &v)) {
            out.push(found(file, line, &name, what));
        }
    }
}

/// Any other text file (config, scripts, styles, docs): known keys anywhere,
/// and `name = "…"` / `"name": "…"` / `name: '…'` assignments.
pub fn text(file: &str, content: &str, out: &mut Vec<Issue>) {
    for (n, line) in content.lines().enumerate() {
        if let Some(what) = known_key(line) {
            out.push(leaked(file, n + 1, what));
            continue;
        }
        for (at, _) in line.match_indices([':', '=']) {
            let key: String = line[..at]
                .trim_end()
                .trim_end_matches(['"', '\''])
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_alphanumeric() || "_-.".contains(*c))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let rest = line[at + 1..].trim_start();
            let Some(q) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else { continue };
            let Some(value) = rest[1..].split(q).next().filter(|_| rest[1..].contains(q)) else { continue };
            if let Some(what) = hardcoded(&key, value) {
                out.push(found(file, n + 1, &key, what));
                break;
            }
        }
    }
}

/// `.env` files are where secrets go: they must not be committed.
pub fn is_env(name: &str) -> bool {
    (name == ".env" || name.starts_with(".env.")) && !name.ends_with(".example") && !name.ends_with(".sample")
}

/// Without git: does `.gitignore` text ignore `.env` files?
pub fn ignores_env(gitignore: &str) -> bool {
    gitignore.lines().map(str::trim).any(|l| [".env", ".env*", "*.env", ".env.*", "/.env"].contains(&l))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::ToTokens;

    #[test]
    fn names_split_into_words() {
        assert_eq!(words("apiKey"), ["api", "key"]);
        assert_eq!(words("private_key"), ["private", "key"]);
        assert_eq!(words("APIKey"), ["api", "key"]);
        assert_eq!(words("SEED_2"), ["seed", "2"]);
        assert_eq!(words("seed2Phrase"), ["seed", "2", "phrase"]);
        assert_eq!(words("monkey"), ["monkey"]);
    }

    #[test]
    fn credential_names_match_aios_refusal_list() {
        for k in [
            "apiKey",
            "api_key",
            "API_KEY",
            "privateKey",
            "private_key",
            "secretKey",
            "accessToken",
            "auth_token",
            "authToken",
            "userPassword",
            "user_password",
            "passphrase",
            "mnemonic",
        ] {
            assert!(credential(k), "{k} is a credential");
        }
        for k in [
            "passwordless",
            "monkey",
            "keyboard",
            "seedling",
            "privacy",
            "publicKey",
            "public_key",
            "pubKey",
            "owner_public_key",
            "apiKeyName",
            "api_key_name",
            "privateKeyId",
            "tokenList",
            "secretSanta",
        ] {
            assert!(!credential(k), "{k} is not a credential");
        }
        // An exemption is a word, not a substring.
        assert!(credential("pubsubSecretKey") && credential("epubPassword"));
    }

    #[test]
    fn hardcoded_values_follow_aios_thresholds() {
        assert_eq!(hardcoded("password", "hunter2"), Some("password"));
        assert_eq!(hardcoded("password", "abc"), None, "too short");
        assert_eq!(hardcoded("token", "abcdefghij0123456789xyz"), Some("token"));
        assert_eq!(hardcoded("token", "short-token"), None);
        assert_eq!(hardcoded("token", "has spaces in it, not a token"), None);
        assert_eq!(hardcoded("client_secret", "s3cr3tvalue"), Some("secret"));
        assert_eq!(hardcoded("API_KEY", "0123456789ab"), Some("key"));
        assert_eq!(hardcoded("password", "{pw}"), None, "a format string");
        assert_eq!(hardcoded("password", "changeme"), None, "a placeholder");
        assert_eq!(hardcoded("token_count", "abcdefghij0123456789xyz"), None, "metadata");
        assert_eq!(hardcoded("public_key", "0123456789abcdef"), None);
        assert_eq!(hardcoded("label", "a password here"), None);
        assert_eq!(hardcoded("HAS_SECRET", "(() => !!document.querySelector('[data-secret]'))()"), None, "code");
        assert_eq!(hardcoded("DISCARD_SECRET", "a sentence about it"), None, "prose");
        assert_eq!(hardcoded("password", "correct horse battery"), Some("password"), "a passphrase has spaces");
    }

    #[test]
    fn known_formats_are_found_on_word_starts_only() {
        assert_eq!(known_key("key = AKIAIOSFODNN7EXAMPLE"), Some("an AWS access key"));
        assert_eq!(known_key(&format!("\"ghp_{}\"", "a1".repeat(18))), Some("a GitHub token"));
        assert_eq!(known_key("sk-ant-api03-abcdef0123456789xyz"), Some("an API secret key"));
        assert_eq!(known_key("-----BEGIN OPENSSH PRIVATE KEY-----"), Some("a private key"));
        assert_eq!(known_key("-----BEGIN PUBLIC KEY-----"), None);
        assert_eq!(known_key("task-list-item-container-wide"), None, "`sk-` inside a word");
        assert_eq!(known_key("sk-this-is-a-css-class-name-x"), None, "no digits: a word");
        assert_eq!(known_key("XAKIAIOSFODNN7EXAMPLE"), None);
    }

    #[test]
    fn rust_tokens_find_assignments_and_keys_but_not_comments() {
        let src = r#"
            // let password = "in a comment";
            const API_KEY: &str = "0123456789abcdef";
            fn f() -> S { S { password: "hunter2".into(), label: "Password" } }
            /// ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1 in a doc comment
            fn g(password: &str) {}
            fn h() { m.insert("api_key", "zz0123456789"); let x = a::b; if t == "x" {} }
        "#;
        let mut out = Vec::new();
        tokens("src/a.rs", syn::parse_file(src).unwrap().into_token_stream(), &mut out);
        let got: Vec<_> = out.iter().map(|i| (i.line.unwrap(), i.severity)).collect();
        assert_eq!(got, [(3, Severity::Warn), (4, Severity::Warn), (5, Severity::Error)], "{out:?}");
    }

    #[test]
    fn text_files_find_toml_json_and_js_assignments() {
        let mut out = Vec::new();
        let src = "[auth]\ntoken = \"abcdefghij0123456789xyz\"\n{\"apiKey\": \"0123456789ab\"}\nconst password = 'hunter22';\nname = \"x\"\nurl: \"https://a\"\n";
        text("aio.toml", src, &mut out);
        assert_eq!(out.iter().map(|i| i.line.unwrap()).collect::<Vec<_>>(), [2, 3, 4], "{out:?}");
    }

    #[test]
    fn env_files_and_gitignore() {
        assert!(is_env(".env") && is_env(".env.local") && !is_env(".env.example") && !is_env("env.rs"));
        assert!(ignores_env("target\n.env\n") && ignores_env(" .env* ") && !ignores_env("target\n"));
    }
}

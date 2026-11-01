//! `aio.toml`'s types and checks, without the runtime: compiled into the app
//! (`aio::config`) and, with `toml`, into `aio::config!()`, which parses and
//! checks the file as the app builds. Its modules are named as aio's, so a
//! config written as Rust ([`config::Lit`]) names `aio::<module>::<type>`.
pub mod budgets;
pub mod config;
pub mod icon;
pub mod perf;
pub mod security;
pub mod vitals;

/// `aio::log`'s level.
pub mod log {
    /// How much a line asks of you: info = nothing to do, warn = should be
    /// fixed, error = must be fixed. Trace and debug are opt-in.
    #[derive(
        Clone,
        Copy,
        Debug,
        Default,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        serde::Deserialize,
        serde::Serialize,
        aio_macros::Lit,
    )]
    #[serde(rename_all = "lowercase")]
    pub enum Level {
        Trace,
        Debug,
        #[default]
        Info,
        Warn,
        Error,
    }
}

/// `aio::auth`'s policy, as `[accounts]` sets it.
pub mod auth {
    /// The longest session a policy may ask for: a century.
    pub const MAX_TTL_MS: i64 = 100 * 365 * 86_400_000;
    /// The live sessions one user may hold unless the app says otherwise.
    pub const MAX_SESSIONS: u32 = 10;

    /// What the app allows, decided once (in `aio.toml`'s `[auth]`).
    #[derive(Clone, Debug)]
    pub struct Policy {
        /// How long a session lives without use.
        pub ttl_ms: i64,
        /// The most live sessions one user may hold; a new one past it ends the oldest.
        pub max_sessions: Option<u32>,
        /// Anyone may create an account (`POST /__aio/auth/signup`). Off: only
        /// the app or its operator makes accounts.
        pub signup: bool,
        /// No sign-in until the account's email is verified (a mailed token,
        /// or the operator's `ax auth verify`); sign-up then needs an email.
        pub require_verified: bool,
    }

    impl Default for Policy {
        fn default() -> Self {
            Self { ttl_ms: 30 * 86_400_000, max_sessions: Some(MAX_SESSIONS), signup: false, require_verified: false }
        }
    }
}

/// `aio::capabilities`' pure half: what a `[capabilities] network` entry is
/// and what it lets through.
pub mod capabilities {
    /// Why `entry` can't be a `[capabilities] network` entry, or `None` (pure).
    pub fn bad_entry(entry: &str) -> Option<String> {
        let host = entry.strip_prefix("*.").unwrap_or(entry);
        let ok = !host.is_empty()
            && !host.contains("://")
            && !host.contains(['/', '?', '#', '@', ' '])
            && (entry == "*" || !host.contains('*'));
        (!ok).then(|| {
            format!(
                "[capabilities] network {entry:?} is not a host: \"api.example.org\", \"10.0.0.5:8443\", \
                 \"*.example.org\", \"lan\" or \"*\""
            )
        })
    }

    /// The host and port of `target`: a URL (`https://h:1/p`) or `host:port`.
    /// Pure.
    pub fn host_port(target: &str) -> (String, Option<u16>) {
        let rest = target.split_once("://").map_or(target, |(_, r)| r);
        let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
        let auth = auth.rsplit_once('@').map_or(auth, |(_, a)| a);
        let (host, port) = match auth.strip_prefix('[') {
            Some(v6) => match v6.split_once(']') {
                Some((h, p)) => (h, p.strip_prefix(':')),
                None => (v6, None),
            },
            None if auth.matches(':').count() == 1 => match auth.split_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (auth, None),
            },
            None => (auth, None),
        };
        (host.trim_end_matches('.').to_ascii_lowercase(), port.and_then(|p| p.parse().ok()))
    }

    /// This machine: loopback addresses and `localhost`.
    fn here(host: &str) -> bool {
        host == "localhost"
            || host.ends_with(".localhost")
            || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
    }

    /// This network: private and link-local addresses, `.local` names.
    fn lan(host: &str) -> bool {
        use std::net::IpAddr;
        host.ends_with(".local")
            || match host.parse::<IpAddr>() {
                Ok(IpAddr::V4(ip)) => ip.is_private() || ip.is_link_local(),
                Ok(IpAddr::V6(ip)) => ip.is_unique_local() || ip.is_unicast_link_local(),
                Err(_) => false,
            }
    }

    /// Whether `network` lets aio reach `host` (lowercase) on `port`. Pure.
    pub fn allows(network: &[String], host: &str, port: Option<u16>) -> bool {
        here(host)
            || network.iter().any(|e| {
                let e = e.to_ascii_lowercase();
                match e.as_str() {
                    "*" => true,
                    "lan" => lan(host),
                    _ => match e.strip_prefix("*.") {
                        Some(d) => host.strip_suffix(d).is_some_and(|sub| sub.ends_with('.')),
                        None => match host_port(&e) {
                            (h, None) => h == host,
                            (h, Some(p)) => h == host && Some(p) == port,
                        },
                    },
                }
            })
    }
}

/// `aio::update`'s pure half: release sources, keys and channel names.
pub mod update {
    pub fn b64() -> &'static base64::engine::GeneralPurpose {
        &base64::engine::general_purpose::STANDARD
    }

    /// `ed25519:<base64>` → the raw public key.
    pub fn raw_key(k: &str) -> Result<Vec<u8>, String> {
        use base64::Engine;
        let b = k.strip_prefix("ed25519:").ok_or_else(|| format!("key {k:.20}…: not ed25519:<base64>"))?;
        let raw = b64().decode(b).map_err(|_| format!("key {k:.20}…: bad base64"))?;
        if raw.len() != 32 {
            return Err(format!("key {k:.20}…: not 32 bytes"));
        }
        Ok(raw)
    }

    /// `<owner>/<repo>` of a `github:` source, if it names one.
    pub fn github_repo(s: &str) -> Option<(&str, &str)> {
        let (owner, repo) = s.split_once('/')?;
        let ok = |p: &str, more: &str| {
            !p.is_empty()
                && p != "."
                && p != ".."
                && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_".contains(c) || more.contains(c))
        };
        (ok(owner, "") && ok(repo, ".")).then_some((owner, repo))
    }

    /// The repository of a git source (`git+https://…`, `git+ssh://…`,
    /// `git+file://…`), if `url` names one: its channel is the branch or tag
    /// followed, and only commits or tags signed by the built-in keys are built.
    pub fn git_remote(url: &str) -> Option<&str> {
        let r = url.strip_prefix("git+")?;
        ["https://", "ssh://", "file://"].iter().any(|s| r.len() > s.len() && r.starts_with(s)).then_some(r)
    }

    /// `key` (`ed25519:<base64>`, or an OpenSSH `ssh-ed25519 <base64> [comment]`
    /// line) as OpenSSH writes it: `ssh-ed25519 <base64>`.
    pub fn ssh_key(key: &str) -> Result<String, String> {
        use base64::Engine;
        let raw = match key.strip_prefix("ssh-ed25519 ") {
            Some(rest) => {
                let blob = b64().decode(rest.split_whitespace().next().unwrap_or("")).map_err(|_| "bad base64")?;
                let raw = blob.strip_prefix(b"\0\0\0\x0bssh-ed25519\0\0\0\x20").filter(|r| r.len() == 32);
                raw.ok_or_else(|| format!("key {key:.24}…: not an ssh-ed25519 key"))?.to_vec()
            }
            None => raw_key(key)?,
        };
        let blob = [b"\0\0\0\x0bssh-ed25519\0\0\0\x20".as_slice(), &raw].concat();
        Ok(format!("ssh-ed25519 {}", b64().encode(blob)))
    }

    /// Why `name` can't be a channel (it is a folder or release tag), if so.
    pub fn channel_refused(name: &str) -> Option<String> {
        let ok = !name.is_empty()
            && name.len() <= 64
            && !name.starts_with('.')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
        (!ok).then(|| format!("{name:?} is not a channel name (letters, digits, - _ . ; at most 64)"))
    }
}

/// `aio::db`'s pragma names.
pub mod db {
    /// The pragma `stmt` sets, lowercased and without its schema
    /// (`PRAGMA main.Synchronous = FULL` → `synchronous`); `None`: not a pragma.
    pub fn pragma_name(stmt: &str) -> Option<String> {
        let p = stmt.trim_start();
        let rest = p.get(..6).filter(|h| h.eq_ignore_ascii_case("pragma")).map(|_| &p[6..])?;
        let n: String = rest.trim_start().chars().take_while(|c| c.is_alphanumeric() || "_.".contains(*c)).collect();
        let n = n.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
        (!n.is_empty()).then_some(n)
    }
}

/// `aio::paths`' app ids.
pub mod paths {
    /// An app id is a folder name: letters, digits, `-`, `_`, `.`, not a dot first.
    pub fn id_ok(id: &str) -> std::io::Result<()> {
        let ok = !id.is_empty()
            && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
            && !id.starts_with('.');
        if !ok {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("app id `{id}`: letters, digits, - _ . only"),
            ));
        }
        Ok(())
    }
}

/// `aio::report`'s defaults.
pub mod report {
    /// Reports kept on disk, newest first.
    pub const KEEP: usize = 50;
}

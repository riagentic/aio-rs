//! Why an update step failed ([`UpdateError`]): one variant per thing a
//! caller can act on. Each one's text is the message it always was, so logs
//! and the `updates` cell's `error` read the same.
use super::FORMAT;
use std::{fmt, io};

/// Why an update step failed: a release refused (its signature, its keys,
/// what it is for), a patch that can't be used, a key that isn't one, the
/// data here unreadable, or the runtime's transport. Its `Display` is the
/// message for a person (`release refused: {e}`).
#[derive(Debug)]
#[non_exhaustive]
pub enum UpdateError {
    /// The signature doesn't hold over what was signed: tampered with, or
    /// a damaged download.
    BadSignature,
    /// Signed by a key the app doesn't trust (not built in, nor vouched for
    /// by a roster it trusts).
    UntrustedKey,
    /// Its key roster isn't signed by a key the app trusts.
    UntrustedRoster,
    /// Its key roster names no key.
    EmptyRoster,
    /// Its patch list isn't signed by the release's own key.
    UnsignedPatches,
    /// The release served at `url` was refused, and why (a runtime's
    /// [`check_served`](super::check_served) of what it downloaded).
    Refused {
        url: String,
        why: Box<UpdateError>,
    },
    /// A manifest format this build doesn't read (a newer `ax release`'s).
    Format(u32),
    /// A release for another app, channel, OS or CPU (`what`: `app`,
    /// `channel`, `os` or `arch`).
    NotForThisApp { what: &'static str, got: String, want: String },
    /// A version or file name that is a path, not a plain name.
    NotPlainName(&'static str),
    /// Its `minFrom` is not a version.
    MinFrom(String),
    /// Bytes that aren't a manifest, or a release (`what`), as JSON.
    Malformed {
        what: &'static str,
        error: serde_json::Error,
    },
    /// The patch list is for another release (or another build of it).
    PatchesForAnotherRelease,
    /// None of the release's patches starts from the version installed: the
    /// whole release is downloaded instead.
    NoPatch { from: String, patches: usize },
    /// The patch is over half the release: not worth it.
    PatchTooBig { size: u64, release: u64 },
    /// A git source has no branch or tag `name`.
    NoRef { name: String, remote_has: String },
    /// A git ref that names something other than a commit id.
    NotACommit { name: String, sha: String },
    /// Not a key: not an Ed25519 PKCS#8 signing key, not `ed25519:<base64>`
    /// or `ssh-ed25519 …`; why, in words.
    KeyFormat(String),
    /// The system's randomness gave no key.
    Keygen,
    /// The store here can't be read (to know its data's versions).
    Store(Box<dyn std::error::Error + Send + Sync>),
    /// JSON that couldn't be written (a roster's keys, a patch list).
    Json(serde_json::Error),
    /// The host answered `url` with HTTP `status` (404: no such release).
    Http { url: String, status: u16 },
    /// `url` couldn't be reached: why, in words.
    Network { url: String, message: String },
    /// A file, a process or a download failed (its kind says how).
    Io(io::Error),
    /// A runtime's own failure (an app's own [`Runtime`](super::Runtime)),
    /// in its words.
    Other(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadSignature => f.write_str("its signature is wrong"),
            Self::UntrustedKey => f.write_str("it is signed by a key this app doesn't trust"),
            Self::UntrustedRoster => f.write_str("its key roster is not signed by a trusted key"),
            Self::EmptyRoster => f.write_str("its key roster is empty"),
            Self::UnsignedPatches => f.write_str("its patch list is not signed by the release's key"),
            Self::Refused { url, why } => write!(f, "release refused: {why} ({url})"),
            Self::Format(n) => write!(f, "manifest format {n} (this build reads {FORMAT})"),
            Self::NotForThisApp { what, got, want } => write!(f, "it is for {what} {got}, not {want}"),
            Self::NotPlainName(what) => write!(f, "its {what} is not a plain name"),
            Self::MinFrom(v) => write!(f, "its minFrom {v:?} is not a version"),
            Self::Malformed { what, error } => write!(f, "not a {what}: {error}"),
            Self::PatchesForAnotherRelease => f.write_str("its patch list is for another release"),
            Self::NoPatch { from, patches } => {
                write!(f, "none of its {patches} patches starts from {from}, the version installed here")
            }
            Self::PatchTooBig { size, release } => {
                write!(f, "its patch is {size} bytes, over half of the whole release ({release})")
            }
            Self::NoRef { name, remote_has } => {
                write!(f, "no branch or tag named exactly {name:?} (the remote has {remote_has})")
            }
            Self::NotACommit { name, sha } => write!(f, "{name}: {sha:?} is not a commit id"),
            Self::KeyFormat(m) | Self::Other(m) => f.write_str(m),
            Self::Keygen => f.write_str("no key generated"),
            Self::Store(e) => e.fmt(f),
            Self::Json(e) => e.fmt(f),
            Self::Http { url, status } => write!(f, "{url}: http status: {status}"),
            Self::Network { url, message } => write!(f, "{url}: {message}"),
            Self::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for UpdateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Refused { why, .. } => Some(why),
            Self::Malformed { error, .. } | Self::Json(error) => Some(error),
            Self::Store(e) => Some(&**e),
            // Transparent: the I/O error's own source, not that error.
            Self::Io(e) => e.source(),
            _ => None,
        }
    }
}

impl UpdateError {
    pub(super) fn not_pkcs8() -> Self {
        Self::KeyFormat("not an Ed25519 PKCS#8 key".into())
    }
    pub(super) fn malformed(what: &'static str, error: serde_json::Error) -> Self {
        Self::Malformed { what, error }
    }
    pub(super) fn store(e: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Store(Box::new(e))
    }
}

/// An `io::Error` carrying an `UpdateError` (`io::Error::other(e)`, as a
/// runtime passes one up through its I/O) is that error again; any other is
/// [`UpdateError::Io`].
impl From<io::Error> for UpdateError {
    fn from(e: io::Error) -> Self {
        if e.get_ref().is_some_and(|i| i.is::<UpdateError>()) {
            if let Some(Ok(u)) = e.into_inner().map(|i| i.downcast::<UpdateError>()) {
                return *u;
            }
            unreachable!("checked to be an UpdateError");
        }
        Self::Io(e)
    }
}

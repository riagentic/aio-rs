//! Why an update step failed ([`UpdateError`]): one variant per thing a
//! caller can act on. Each one's text is the message it always was, so logs
//! and the `updates` cell's `error` read the same.
use super::FORMAT;
use std::io;

/// Why an update step failed: a release refused (its signature, its keys,
/// what it is for), a patch that can't be used, a key that isn't one, the
/// data here unreadable, or the runtime's transport. Its `Display` is the
/// message for a person (`release refused: {e}`).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UpdateError {
    /// The signature doesn't hold over what was signed: tampered with, or
    /// a damaged download.
    #[error("its signature is wrong")]
    BadSignature,
    /// Signed by a key the app doesn't trust (not built in, nor vouched for
    /// by a roster it trusts).
    #[error("it is signed by a key this app doesn't trust")]
    UntrustedKey,
    /// Its key roster isn't signed by a key the app trusts.
    #[error("its key roster is not signed by a trusted key")]
    UntrustedRoster,
    /// Its key roster names no key.
    #[error("its key roster is empty")]
    EmptyRoster,
    /// Its patch list isn't signed by the release's own key.
    #[error("its patch list is not signed by the release's key")]
    UnsignedPatches,
    /// The release served at `url` was refused, and why (a runtime's
    /// [`check_served`](super::check_served) of what it downloaded).
    #[error("release refused: {why} ({url})")]
    Refused {
        url: String,
        #[source]
        why: Box<UpdateError>,
    },
    /// A manifest format this build doesn't read (a newer `ax release`'s).
    #[error("manifest format {0} (this build reads {FORMAT})")]
    Format(u32),
    /// A release for another app, channel, OS or CPU (`what`: `app`,
    /// `channel`, `os` or `arch`).
    #[error("it is for {what} {got}, not {want}")]
    NotForThisApp { what: &'static str, got: String, want: String },
    /// A version or file name that is a path, not a plain name.
    #[error("its {0} is not a plain name")]
    NotPlainName(&'static str),
    /// Its `minFrom` is not a version.
    #[error("its minFrom {0:?} is not a version")]
    MinFrom(String),
    /// Bytes that aren't a manifest, or a release (`what`), as JSON.
    #[error("not a {what}: {error}")]
    Malformed {
        what: &'static str,
        #[source]
        error: serde_json::Error,
    },
    /// The patch list is for another release (or another build of it).
    #[error("its patch list is for another release")]
    PatchesForAnotherRelease,
    /// None of the release's patches starts from the version installed: the
    /// whole release is downloaded instead.
    #[error("none of its {patches} patches starts from {from}, the version installed here")]
    NoPatch { from: String, patches: usize },
    /// The patch is over half the release: not worth it.
    #[error("its patch is {size} bytes, over half of the whole release ({release})")]
    PatchTooBig { size: u64, release: u64 },
    /// A git source has no branch or tag `name`.
    #[error("no branch or tag named exactly {name:?} (the remote has {remote_has})")]
    NoRef { name: String, remote_has: String },
    /// A git ref that names something other than a commit id.
    #[error("{name}: {sha:?} is not a commit id")]
    NotACommit { name: String, sha: String },
    /// Not a key: not an Ed25519 PKCS#8 signing key, not `ed25519:<base64>`
    /// or `ssh-ed25519 …`; why, in words.
    #[error("{0}")]
    KeyFormat(String),
    /// The system's randomness gave no key.
    #[error("no key generated")]
    Keygen,
    /// The store here can't be read (to know its data's versions).
    #[error("{0}")]
    Store(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// JSON that couldn't be written (a roster's keys, a patch list).
    #[error("{0}")]
    Json(#[source] serde_json::Error),
    /// The host answered `url` with HTTP `status` (404: no such release).
    #[error("{url}: http status: {status}")]
    Http { url: String, status: u16 },
    /// `url` couldn't be reached: why, in words.
    #[error("{url}: {message}")]
    Network { url: String, message: String },
    /// A file, a process or a download failed (its kind says how).
    #[error(transparent)]
    Io(io::Error),
    /// A runtime's own failure (an app's own [`Runtime`](super::Runtime)),
    /// in its words.
    #[error("{0}")]
    Other(String),
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

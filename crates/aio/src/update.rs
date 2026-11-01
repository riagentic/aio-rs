//! Updates, the verifiable part: release manifests, their Ed25519
//! signatures and key rosters, versions, the data gate, and the app-facing
//! `updates` cell ([`Updates`], on a [`Runtime`]). No network here:
//! `aio_shell::update` fetches and installs, `ax release` signs.
//!
//! Trust is built in: the keys in the app's `aio.toml` (`[updates] keys`),
//! never the first key a server shows. A release may carry a roster, the
//! keys to trust from now on, signed by a key trusted already: that is how a
//! key is rotated or retired. Every field of a manifest is signed.
//!
//! A release may have patches (delta updates): the files that turn an
//! earlier version into it, listed apart (`<platform>.patches.json`, so a
//! build that reads none updates as before) and signed by the release's own
//! key for that very release ([`Patches`]).
use ring::signature::{self, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

mod cell;
pub use cell::{
    Blocked, Data, Found, Offer, Phase, Ready, Runtime, Step, Stub, Updates, UpdatesCalls, Veto, can_apply, install,
};

/// The manifest format this build reads.
pub const FORMAT: u32 = 1;

/// What a build can read: the store layout and each cell's data version.
/// Any older version of a cell migrates (`CellData::migrate`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub schema: u32,
    /// Cell id → its data version.
    pub cells: BTreeMap<String, u32>,
}

/// A release: everything in it is signed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Release {
    pub format: u32,
    pub id: String,
    pub version: String,
    pub channel: String,
    pub os: String,
    pub arch: String,
    /// The bundle, relative to the manifest (same origin, always).
    pub file: String,
    pub sha256: String,
    pub size: u64,
    pub released_at: String,
    pub notes: String,
    pub data: Contract,
    pub roster: Option<Roster>,
}

/// The keys to trust from this release on, signed by keys trusted already.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roster {
    pub keys: Vec<String>,
    /// `[key, signature of keys]` pairs: one is enough if its key is trusted.
    pub signatures: Vec<[String; 2]>,
}

/// A release and its signature, as the manifest file holds them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub release: Release,
    /// The signing key (`ed25519:<base64>`) and its signature.
    pub key: String,
    pub signature: String,
}

/// The bytes a release's signature covers: its fields in their fixed order.
fn signed_bytes(r: &Release) -> Vec<u8> {
    serde_json::to_vec(r).unwrap_or_default()
}

use aio_config::update::{b64, raw_key};
/// Whether `sig` (base64) is `key`'s (`ed25519:…`) signature of `msg`.
pub fn verify(key: &str, msg: &[u8], sig: &str) -> bool {
    use base64::Engine;
    let (Ok(raw), Ok(sig)) = (raw_key(key), b64().decode(sig)) else { return false };
    UnparsedPublicKey::new(&signature::ED25519, raw).verify(msg, &sig).is_ok()
}

/// A new signing key: (its PKCS#8 bytes, to keep secret; its public key).
pub fn keygen() -> Result<(Vec<u8>, String), String> {
    use base64::Engine;
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| "no key generated")?;
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| "bad key")?;
    Ok((pkcs8.as_ref().to_vec(), format!("ed25519:{}", b64().encode(pair.public_key().as_ref()))))
}

/// The public key of a PKCS#8 signing key.
pub fn public_key(pkcs8: &[u8]) -> Result<String, String> {
    use base64::Engine;
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| "not an Ed25519 PKCS#8 key")?;
    Ok(format!("ed25519:{}", b64().encode(pair.public_key().as_ref())))
}

/// `msg` signed with `pkcs8`: the signature, base64.
pub fn sign_with(pkcs8: &[u8], msg: &[u8]) -> Result<String, String> {
    use base64::Engine;
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| "not an Ed25519 PKCS#8 key")?;
    Ok(b64().encode(pair.sign(msg).as_ref()))
}

/// A roster of `keys`, signed by each of `signers` (PKCS#8).
pub fn roster(keys: &[String], signers: &[Vec<u8>]) -> Result<Roster, String> {
    let msg = serde_json::to_vec(keys).map_err(|e| e.to_string())?;
    let signatures =
        signers.iter().map(|s| Ok([public_key(s)?, sign_with(s, &msg)?])).collect::<Result<Vec<_>, String>>()?;
    Ok(Roster { keys: keys.to_vec(), signatures })
}

/// `release` signed with `pkcs8`.
pub fn sign(release: Release, pkcs8: &[u8]) -> Result<Manifest, String> {
    let signature = sign_with(pkcs8, &signed_bytes(&release))?;
    Ok(Manifest { key: public_key(pkcs8)?, signature, release })
}

/// The keys trusted after a roster: its keys, if a key in `trusted` signed it.
pub fn follow(trusted: &[String], roster: &Roster) -> Result<Vec<String>, String> {
    let msg = serde_json::to_vec(&roster.keys).map_err(|e| e.to_string())?;
    let vouched = roster.signatures.iter().any(|[k, s]| trusted.contains(k) && verify(k, &msg, s));
    if !vouched {
        return Err("its key roster is not signed by a trusted key".into());
    }
    if roster.keys.is_empty() {
        return Err("its key roster is empty".into());
    }
    Ok(roster.keys.clone())
}

/// What this app expects of a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expect<'a> {
    pub id: &'a str,
    pub channel: &'a str,
    pub os: &'a str,
    pub arch: &'a str,
}

/// Check a manifest against the trusted keys: the release, and the keys to
/// trust from now on (the roster's, if it carries one).
pub fn check(m: &Manifest, trusted: &[String], expect: &Expect) -> Result<Vec<String>, String> {
    let r = &m.release;
    if r.format != FORMAT {
        return Err(format!("manifest format {} (this build reads {FORMAT})", r.format));
    }
    for (what, got, want) in [
        ("app", &r.id, expect.id),
        ("channel", &r.channel, expect.channel),
        ("os", &r.os, expect.os),
        ("arch", &r.arch, expect.arch),
    ] {
        if got != want {
            return Err(format!("it is for {what} {got}, not {want}"));
        }
    }
    if !safe_name(&r.version) || !safe_name(&r.file) {
        return Err("its version or file name is not a plain name".into());
    }
    let next = match &r.roster {
        Some(roster) => follow(trusted, roster)?,
        None => trusted.to_vec(),
    };
    if !next.contains(&m.key) {
        return Err("it is signed by a key this app doesn't trust".into());
    }
    if !verify(&m.key, &signed_bytes(r), &m.signature) {
        return Err("its signature is wrong".into());
    }
    Ok(next)
}

/// A patch: the file that makes a release from version `from` installed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub from: String,
    /// Beside the manifest, as the bundle.
    pub file: String,
    pub size: u64,
    pub sha256: String,
}

/// The patches to one release (its id, version and bundle's SHA-256).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchList {
    pub id: String,
    pub version: String,
    pub sha256: String,
    pub patches: Vec<Patch>,
}

/// A patch list and its signature, as `<platform>.patches.json` holds them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patches {
    pub list: PatchList,
    pub key: String,
    pub signature: String,
}

/// `patches` to `m`'s release, signed with `pkcs8` (the key that signed `m`).
pub fn sign_patches(m: &Manifest, patches: Vec<Patch>, pkcs8: &[u8]) -> Result<Patches, String> {
    let r = &m.release;
    let list = PatchList { id: r.id.clone(), version: r.version.clone(), sha256: r.sha256.clone(), patches };
    let signature = sign_with(pkcs8, &serde_json::to_vec(&list).map_err(|e| e.to_string())?)?;
    Ok(Patches { list, key: public_key(pkcs8)?, signature })
}

/// The patch to `m` (checked already) from `installed`: signed by `m`'s key
/// for `m`'s very release, and at most half its size (more is not worth it).
pub fn patch_for<'a>(p: &'a Patches, m: &Manifest, installed: &str) -> Result<&'a Patch, String> {
    let (r, l) = (&m.release, &p.list);
    if p.key != m.key || !verify(&p.key, &serde_json::to_vec(l).unwrap_or_default(), &p.signature) {
        return Err("its patch list is not signed by the release's key".into());
    }
    if (&l.id, &l.version, &l.sha256) != (&r.id, &r.version, &r.sha256) {
        return Err("its patch list is for another release".into());
    }
    let patch = l.patches.iter().find(|x| x.from == installed).ok_or_else(|| {
        format!("none of its {} patches starts from {installed}, the version installed here", l.patches.len())
    })?;
    if !safe_name(&patch.file) {
        return Err("its patch's file name is not a plain name".into());
    }
    if patch.size.saturating_mul(2) > r.size {
        return Err(format!("its patch is {} bytes, over half of the whole release ({})", patch.size, r.size));
    }
    Ok(patch)
}

/// Letters, digits, `.`, `-`, `_`, `+`, not starting with `.`: no paths.
fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with('.')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-+".contains(c))
}

/// A version: `1.2.3`, `1.2.3-alpha.4`; `+build` ignored. `None`: not one.
pub fn parse_version(v: &str) -> Option<(Vec<u64>, Option<String>)> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let v = v.split('+').next()?;
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) if !p.is_empty() => (c, Some(p.to_string())),
        Some(_) => return None,
        None => (v, None),
    };
    let mut nums: Vec<u64> = core.split('.').map(|n| n.parse().ok()).collect::<Option<_>>()?;
    if nums.is_empty() || nums.len() > 3 {
        return None;
    }
    nums.resize(3, 0);
    Some((nums, pre))
}

/// `a` against `b`: numbers first; a release above its prereleases; digit
/// runs in a prerelease compare as numbers (`alpha9 < alpha62`).
pub fn compare(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering::*;
    let ((an, ap), (bn, bp)) = (parse_version(a)?, parse_version(b)?);
    Some(an.cmp(&bn).then_with(|| match (ap, bp) {
        (None, None) => Equal,
        (None, Some(_)) => Greater,
        (Some(_), None) => Less,
        (Some(x), Some(y)) => runs(&x).cmp(&runs(&y)),
    }))
}

fn runs(s: &str) -> Vec<(u8, u64, String)> {
    let mut out = vec![];
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        let digit = c.is_ascii_digit();
        let mut run = String::new();
        while let Some(&d) = chars.peek() {
            if d.is_ascii_digit() != digit {
                break;
            }
            run.push(d);
            chars.next();
        }
        out.push(if digit { (0, run.parse().unwrap_or(u64::MAX), String::new()) } else { (1, 0, run) });
    }
    out
}

fn is_pre(v: &str) -> bool {
    parse_version(v).is_some_and(|(_, p)| p.is_some())
}

/// Does an app on `current` follow prereleases? `prerelease` (aio.toml)
/// says; unset, only off `stable` or once on a prerelease.
pub fn follows_prereleases(prerelease: Option<bool>, current: &str, channel: &str) -> bool {
    prerelease.unwrap_or(is_pre(current) || channel != "stable")
}

/// Should `release` be offered to `current`? Only a newer one, and a
/// prerelease only to an app that follows them.
pub fn newer(release: &str, current: &str, prereleases: bool) -> bool {
    compare(release, current) == Some(std::cmp::Ordering::Greater) && (prereleases || !is_pre(release))
}

/// A newer prerelease that isn't offered: why, naming the switch.
pub fn skipped_prerelease(release: &str, current: &str, prereleases: bool) -> Option<String> {
    (!prereleases && is_pre(release) && compare(release, current) == Some(std::cmp::Ordering::Greater)).then(|| {
        format!("{release} is a prerelease, not offered here ([updates] prerelease = true follows them); {current} is current")
    })
}

pub use aio_config::update::{channel_refused, git_remote, github_repo, ssh_key};
/// The commit `name` is, from `git ls-remote <remote> <name> <name>^{}`:
/// exactly the branch, else the tag (the commit an annotated tag points at),
/// else a full ref name (`HEAD`), the order `git clone --branch` takes them.
/// A ref that only ends with `name` (`feature/main`) is never it.
pub fn git_ref_commit(ls_remote: &str, name: &str) -> Result<String, String> {
    let lines: Vec<(&str, &str)> = ls_remote.lines().filter_map(|l| l.split_once(char::is_whitespace)).collect();
    let find = |r: &str| lines.iter().find(|(_, n)| n.trim() == r).map(|(sha, _)| *sha);
    let full = [format!("refs/heads/{name}"), format!("refs/tags/{name}"), name.to_string()];
    let sha = full.iter().find_map(|r| find(&format!("{r}^{{}}")).or_else(|| find(r))).ok_or_else(|| {
        let seen: Vec<&str> = lines.iter().map(|(_, n)| n.trim()).collect();
        format!("no branch or tag named exactly {name:?} (the remote has {})", seen.join(", "))
    })?;
    match sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        true => Ok(sha.to_ascii_lowercase()),
        false => Err(format!("{name}: {sha:?} is not a commit id")),
    }
}

/// `key`'s OpenSSH fingerprint (`SHA256:<base64>`), as git names a signer.
pub fn ssh_fingerprint(key: &str) -> Result<String, String> {
    use base64::Engine;
    let ssh = ssh_key(key)?;
    let blob = b64().decode(&ssh["ssh-ed25519 ".len()..]).map_err(|_| "bad base64")?;
    let digest = ring::digest::digest(&ring::digest::SHA256, &blob);
    Ok(format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)))
}

/// `keys` as git's SSH allowed-signers file: the only keys whose signed
/// commits and tags a git source builds.
pub fn allowed_signers(keys: &[String]) -> Result<String, String> {
    keys.iter().map(|k| Ok(format!("aio-release namespaces=\"git\" {}\n", ssh_key(k)?))).collect()
}

/// The channel `ax release --channel` built this binary for, if any.
pub const STAMPED_CHANNEL: Option<&str> = option_env!("AIORT_CHANNEL");

/// The channel an app follows, most specific first: `AIORT_UPDATE_CHANNEL`
/// (this run), the user's choice (`set_channel`, this install), the build's
/// stamp (what this binary is: a beta build never updates itself into
/// stable), then `aio.toml`.
pub fn channel<'a>(env: Option<&'a str>, pinned: Option<&'a str>, stamp: Option<&'a str>, config: &'a str) -> &'a str {
    [env, pinned, stamp].into_iter().flatten().find(|c| channel_refused(c).is_none()).unwrap_or(config)
}

/// Where `channel`'s manifests and bundles are (one folder, flat). A
/// `github:<owner>/<repo>` source reads its GitHub releases: `stable` is the
/// latest release, any other channel the release tagged with its name.
pub fn release_dir(url: &str, channel: &str) -> String {
    match url.strip_prefix("github:").and_then(github_repo) {
        Some((owner, repo)) if channel == "stable" => {
            format!("https://github.com/{owner}/{repo}/releases/latest/download")
        }
        Some((owner, repo)) => format!("https://github.com/{owner}/{repo}/releases/download/{channel}"),
        None => format!("{}/{channel}", url.trim_end_matches('/')),
    }
}

/// The data on this machine: the store's layout and each cell's newest
/// data version (`None`: no store yet).
pub fn local_data(store: &Path) -> Result<Option<Contract>, String> {
    if !store.exists() {
        return Ok(None);
    }
    let db = rusqlite::Connection::open_with_flags(store, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let schema: u32 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    let mut q = db
        .prepare("SELECT cell, MAX(v) FROM (SELECT cell, v FROM snap UNION ALL SELECT cell, v FROM log) GROUP BY cell")
        .map_err(|e| e.to_string())?;
    let cells = q
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    Ok(Some(Contract { schema, cells }))
}

/// Why a build with `contract` can't take over `local` data: none,
/// or each reason. A cell the release no longer has is not a blocker (its
/// data stays in the store), only said.
pub fn blockers(contract: &Contract, local: &Contract) -> Vec<String> {
    let mut why = vec![];
    if contract.schema < local.schema {
        why.push(format!("the store's layout is {}, the release reads up to {}", local.schema, contract.schema));
    }
    for (cell, &have) in &local.cells {
        match contract.cells.get(cell) {
            Some(&reads) if reads < have => {
                why.push(format!("{cell}: the data is v{have}, the release reads up to v{reads}"))
            }
            _ => {}
        }
    }
    why
}

/// The cells this process opened, with their versions: its contract.
static OPENED: std::sync::Mutex<BTreeMap<String, u32>> = std::sync::Mutex::new(BTreeMap::new());

/// A store opened a cell (so this build reads its data).
pub(crate) fn opened(id: &str, version: u32) {
    OPENED.lock().unwrap_or_else(|e| e.into_inner()).insert(id.into(), version);
}

/// This build's contract: the cells it opened, and its store layout.
pub fn contract() -> Contract {
    Contract { schema: crate::store::SCHEMA as u32, cells: OPENED.lock().unwrap_or_else(|e| e.into_inner()).clone() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::*;

    fn release(version: &str) -> Release {
        Release {
            format: FORMAT,
            id: "notes".into(),
            version: version.into(),
            channel: "stable".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            file: format!("notes-{version}-linux-x64.tar.gz"),
            sha256: "ab".repeat(32),
            size: 10,
            released_at: "2026-10-28T00:00:00.000Z".into(),
            notes: "".into(),
            data: Contract::default(),
            roster: None,
        }
    }

    const EXPECT: Expect = Expect { id: "notes", channel: "stable", os: "linux", arch: "x86_64" };

    #[test]
    fn a_patch_is_used_only_under_the_releases_key_for_that_release() {
        let (k, _) = keygen().unwrap();
        let (other, _) = keygen().unwrap();
        let m = sign(release("1.1.0"), &k).unwrap();
        let patch = |from: &str, size| Patch {
            from: from.into(),
            file: format!("notes-{from}-to-1.1.0.patch"),
            size,
            sha256: "cd".repeat(32),
        };
        let p = sign_patches(&m, vec![patch("1.0.0", 4), patch("0.9.0", 5)], &k).unwrap();
        assert_eq!(patch_for(&p, &m, "1.0.0"), Ok(&patch("1.0.0", 4)));
        assert!(patch_for(&p, &m, "1.0.1").unwrap_err().contains("none of its 2 patches starts from 1.0.1"));
        // Over half the release is not worth it.
        let big = sign_patches(&m, vec![patch("1.0.0", 6)], &k).unwrap();
        assert!(patch_for(&big, &m, "1.0.0").unwrap_err().contains("over half"));
        // Another key, an edited list, or the list of another build of the release: not used.
        let foreign = sign_patches(&m, vec![patch("1.0.0", 4)], &other).unwrap();
        assert!(patch_for(&foreign, &m, "1.0.0").unwrap_err().contains("not signed"));
        let mut edited = p.clone();
        edited.list.patches[0].sha256 = "ef".repeat(32);
        assert!(patch_for(&edited, &m, "1.0.0").unwrap_err().contains("not signed"));
        let rebuilt = sign(Release { sha256: "00".repeat(32), ..release("1.1.0") }, &k).unwrap();
        assert!(patch_for(&p, &rebuilt, "1.0.0").unwrap_err().contains("another release"));
        // A file name is a name, never a path.
        let mut bad = patch("1.0.0", 4);
        bad.file = "../x.patch".into();
        let bad = sign_patches(&m, vec![bad], &k).unwrap();
        assert!(patch_for(&bad, &m, "1.0.0").unwrap_err().contains("plain name"));
    }

    #[test]
    fn only_a_trusted_key_signing_every_field_passes() {
        let (k, public) = keygen().unwrap();
        let (other, _) = keygen().unwrap();
        let m = sign(release("1.1.0"), &k).unwrap();
        assert_eq!(check(&m, std::slice::from_ref(&public), &EXPECT), Ok(vec![public.clone()]));
        // No trust on first use: a key the app wasn't built with fails.
        let foreign = sign(release("1.1.0"), &other).unwrap();
        assert!(check(&foreign, std::slice::from_ref(&public), &EXPECT).unwrap_err().contains("doesn't trust"));
        // Any field changed after signing breaks it.
        for change in [
            |r: &mut Release| r.sha256 = "cd".repeat(32),
            |r: &mut Release| r.notes = "hi".into(),
            |r: &mut Release| r.size = 11,
        ] {
            let mut t = m.clone();
            change(&mut t.release);
            assert_eq!(check(&t, std::slice::from_ref(&public), &EXPECT), Err("its signature is wrong".into()));
        }
        // Another app's, channel's or machine's release is refused.
        let mut wrong = release("1.1.0");
        wrong.arch = "aarch64".into();
        let w = sign(wrong, &k).unwrap();
        assert!(check(&w, std::slice::from_ref(&public), &EXPECT).unwrap_err().contains("arch"));
        // A file name is never a path.
        let mut sneaky = release("1.1.0");
        sneaky.file = "../../.bashrc".into();
        assert!(check(&sign(sneaky, &k).unwrap(), &[public], &EXPECT).is_err());
    }

    #[test]
    fn keys_rotate_through_a_roster_signed_by_a_trusted_key() {
        let (old, old_pub) = keygen().unwrap();
        let (new, new_pub) = keygen().unwrap();
        let built_in = vec![old_pub.clone()];
        // Release N (signed by the old key) vouches for [old, new].
        let mut n = release("1.1.0");
        n.roster = Some(roster(&[old_pub.clone(), new_pub.clone()], std::slice::from_ref(&old)).unwrap());
        let trusted = check(&sign(n, &old).unwrap(), &built_in, &EXPECT).unwrap();
        assert_eq!(trusted, vec![old_pub.clone(), new_pub.clone()]);
        // Release N+1, signed by the new key, retires the old one.
        let mut n1 = release("1.2.0");
        n1.roster = Some(roster(std::slice::from_ref(&new_pub), std::slice::from_ref(&new)).unwrap());
        assert_eq!(check(&sign(n1.clone(), &new).unwrap(), &trusted, &EXPECT), Ok(vec![new_pub.clone()]));
        // An app that only trusts the old key can't be talked into the new one.
        assert!(check(&sign(n1, &new).unwrap(), &built_in, &EXPECT).unwrap_err().contains("roster"));
        // A roster signed by an untrusted key is refused.
        let (evil, evil_pub) = keygen().unwrap();
        let mut e = release("1.3.0");
        e.roster = Some(roster(&[evil_pub], std::slice::from_ref(&evil)).unwrap());
        assert!(check(&sign(e, &evil).unwrap(), &built_in, &EXPECT).is_err());
    }

    #[test]
    fn versions_compare_as_people_mean_them() {
        assert_eq!(compare("1.2.10", "1.2.9"), Some(Greater));
        assert_eq!(compare("v1.2", "1.2.0+build7"), Some(Equal));
        assert_eq!(compare("2.0.0", "2.0.0-rc.1"), Some(Greater));
        assert_eq!(compare("2.0.0-alpha9", "2.0.0-alpha62"), Some(Less));
        assert_eq!(compare("1.x", "1.0"), None, "not a version: never 0.0.0");
        let offered = |r, c, said, ch| newer(r, c, follows_prereleases(said, c, ch));
        assert!(offered("1.1.0", "1.0.0", None, "stable"));
        assert!(!offered("1.0.0", "1.1.0", None, "stable"), "never a downgrade");
        assert!(!offered("1.0.0", "1.0.0", None, "stable"));
        assert!(!offered("1.1.0-beta.1", "1.0.0", None, "stable"), "no prerelease on stable");
        assert!(offered("1.1.0-beta.1", "1.0.0", None, "beta"));
        assert!(offered("1.1.0-beta.2", "1.1.0-beta.1", None, "stable"), "a prerelease follows prereleases");
        // aio.toml's word wins, both ways.
        assert!(offered("1.1.0-rc.1", "1.0.0", Some(true), "stable"));
        assert!(!offered("1.1.0-beta.2", "1.1.0-beta.1", Some(false), "beta"));
        assert!(offered("1.1.0", "1.0.0", Some(false), "beta"), "a release is always offered");
        // A skipped prerelease is said, with the switch.
        assert!(skipped_prerelease("1.1.0-rc.1", "1.0.0", false).unwrap().contains("prerelease = true"));
        assert_eq!(skipped_prerelease("1.1.0-rc.1", "1.0.0", true), None);
        assert_eq!(skipped_prerelease("0.9.0-rc.1", "1.0.0", false), None);
    }

    #[test]
    fn a_source_is_a_folder_or_a_github_repositorys_releases() {
        assert_eq!(release_dir("https://x.example/notes/", "beta"), "https://x.example/notes/beta");
        assert_eq!(release_dir("github:me/notes", "stable"), "https://github.com/me/notes/releases/latest/download");
        assert_eq!(release_dir("github:me/notes", "beta"), "https://github.com/me/notes/releases/download/beta");
        assert_eq!(github_repo("my_org-1/app.rs"), Some(("my_org-1", "app.rs")), "EMU owners have `_`");
        for bad in ["me", "me/", "/notes", "me/../x", "me/..", "m e/notes", "me/no?tes"] {
            assert_eq!(github_repo(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_git_source_is_a_repository_and_its_ref_is_resolved_exactly() {
        assert_eq!(git_remote("git+https://git.example/me/notes.git"), Some("https://git.example/me/notes.git"));
        assert_eq!(git_remote("git+ssh://git@git.example:2222/me/notes"), Some("ssh://git@git.example:2222/me/notes"));
        assert_eq!(git_remote("git+file:///srv/notes.git"), Some("file:///srv/notes.git"));
        for bad in ["https://x.example/notes", "git+http://x.example/n", "git+https://", "git+ext::sh -c x", "github:me/n"] {
            assert_eq!(git_remote(bad), None, "{bad}");
        }
        let (a, b, c) = ("a".repeat(40), "b".repeat(40), "c".repeat(40));
        // `main` also matches the tail of `feature/main`, listed first.
        let out = format!("{a}\trefs/heads/feature/main\n{b}\trefs/heads/main\n");
        assert_eq!(git_ref_commit(&out, "main").unwrap(), b);
        // An annotated tag: its commit, never the tag object.
        let tag = format!("{a}\trefs/tags/v1\n{c}\trefs/tags/v1^{{}}\n");
        assert_eq!(git_ref_commit(&tag, "v1").unwrap(), c);
        assert_eq!(git_ref_commit(&format!("{a}\trefs/tags/v2\n"), "v2").unwrap(), a, "a light tag");
        assert_eq!(git_ref_commit(&format!("{c}\tHEAD\n"), "HEAD").unwrap(), c);
        let e = git_ref_commit(&format!("{a}\trefs/heads/feature/main\n"), "main").unwrap_err();
        assert!(e.contains("exactly \"main\"") && e.contains("refs/heads/feature/main"), "{e}");
        assert!(git_ref_commit("", "main").is_err());
        assert!(git_ref_commit("xyz\trefs/heads/main\n", "main").unwrap_err().contains("not a commit"));
    }

    #[test]
    fn the_built_in_keys_are_git_s_allowed_signers() {
        let (_, key) = keygen().unwrap();
        let ssh = ssh_key(&key).unwrap();
        assert!(ssh.starts_with("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI"), "{ssh}");
        assert_eq!(ssh_key(&format!("{ssh} me@laptop")).unwrap(), ssh, "an OpenSSH line, its comment dropped");
        let file = allowed_signers(&[key, ssh.clone()]).unwrap();
        assert_eq!(file, format!("aio-release namespaces=\"git\" {ssh}\n").repeat(2));
        assert!(ssh_key("ssh-rsa AAAAB3Nza").is_err() && ssh_key("ed25519:AAAA").is_err());
        assert!(ssh_key("ssh-ed25519 AAAAB3NzaC1yc2EAAAADAQABAAABAQ").is_err(), "an RSA blob");
        // What `ssh-keygen -lf` says of this public key.
        let known = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOJMjphIy8VC46jSmLLOF0IVOSFhZWWgzoQR5HyF8OAi";
        assert_eq!(ssh_fingerprint(known).unwrap(), "SHA256:W2NbDDTcHYu3mokmQLutYWPMJld/KesdDoLyVMQkEsI");
    }

    #[test]
    fn a_release_that_cant_read_the_data_here_is_blocked() {
        let c = |schema, cells: &[(&str, u32)]| Contract {
            schema,
            cells: cells.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        };
        let local = c(1, &[("notes", 3), ("old", 1)]);
        assert!(blockers(&c(1, &[("notes", 3)]), &local).is_empty(), "a dropped cell keeps its data");
        assert!(blockers(&c(1, &[("notes", 4)]), &local).is_empty(), "a newer version migrates");
        assert_eq!(blockers(&c(1, &[("notes", 2)]), &local), vec!["notes: the data is v3, the release reads up to v2"]);
        assert_eq!(blockers(&c(0, &[("notes", 3)]), &local).len(), 1);
    }

    #[test]
    fn local_data_is_read_without_touching_the_store() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("state.db");
        assert_eq!(local_data(&f), Ok(None));
        let db = rusqlite::Connection::open(&f).unwrap();
        db.execute_batch(
            "CREATE TABLE snap (cell TEXT, seq INTEGER, v INTEGER, state TEXT);
             CREATE TABLE log (seq INTEGER, cell TEXT, v INTEGER);
             INSERT INTO snap VALUES ('notes', 1, 2, '{}');
             INSERT INTO log VALUES (2, 'notes', 3);
             PRAGMA user_version = 1;",
        )
        .unwrap();
        drop(db);
        let l = local_data(&f).unwrap().unwrap();
        assert_eq!((l.schema, l.cells.get("notes")), (1, Some(&3)));
    }
}

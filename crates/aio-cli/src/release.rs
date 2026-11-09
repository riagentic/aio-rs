//! Releases: `ax keygen <app>` makes the app's signing key (kept in
//! `~/.config/aio-rt/keys/<id>.key`, mode 0600, never in the repo) and says
//! the line for `aio.toml`; `ax release <app>` builds the bundle, asks it
//! which data it reads, signs the manifest and lays both out in
//! `dist/<channel>/`, ready to upload to `[updates] url`. With
//! `--patch-from 1.0.0,1.0.1` it also makes the patches that turn those
//! releases (their bundles kept in `dist/<channel>/`) into this one, signed
//! in `<os>-<arch>.patches.json` (`aio_shell::delta`): an app that runs one
//! of them downloads kilobytes instead of the whole bundle.
//!
//! As aio's `am publish`: `--dir` lays the channel out elsewhere,
//! `--platforms` releases for each, `--no-build` releases what `ax build`
//! left in `dist/`, `--data` gives what it reads from a contract file.
//!
//! `ax release <app> --workflow` writes the GitHub Actions workflow that
//! does this on a `v*` tag, for Linux, Windows and macOS, and publishes the
//! files to the tag's GitHub release ([`workflow`]).
//!
//! `ax release --aio [--windows] [--target t] [--dir d] [--url u]` makes aio-rs itself
//! prebuilt ([`aio`]): an archive per platform, `SHA256SUMS`, and
//! `install.sh` / `install.ps1` with each archive's SHA-256 pinned, which
//! check it before anything runs and fall back to a source build by name.
//!
//! Rotating a key: `ax keygen <app> --rotate` keeps the old key as
//! `<id>.previous.key`; put both public keys in `aio.toml` and release. Each
//! release carries a roster of `aio.toml`'s keys signed by every key here, so
//! apps that trust either key move on to the new set.
use aio::update as up;
use std::path::{Path, PathBuf};

/// `~/.config/aio-rt/keys`.
pub fn keys_dir() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    config.join("aio-rt/keys")
}

pub fn read_key(p: &Path) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|_| format!("{}: not a key file", p.display()))
}

fn write_key(p: &Path, pkcs8: &[u8]) -> Result<(), String> {
    use base64::Engine;
    let text = base64::engine::general_purpose::STANDARD.encode(pkcs8);
    aio::paths::write_atomic(p, text.as_bytes(), true).map_err(|e| e.to_string())
}

/// Make the signing key `id` in `dir` (`--rotate`: the old one kept as
/// `<id>.previous.key`): its file, and the public keys to trust.
fn new_key(dir: &Path, id: &str, rotate: bool) -> Result<(PathBuf, Vec<String>), String> {
    aio::paths::private_dir(dir).map_err(|e| e.to_string())?;
    let (key, previous) = (dir.join(format!("{id}.key")), dir.join(format!("{id}.previous.key")));
    let mut keys = vec![];
    if key.exists() {
        if !rotate {
            return Err(format!("{} exists: keep it (losing it ends updates), or --rotate", key.display()));
        }
        let old = read_key(&key)?;
        keys.push(up::public_key(&old)?);
        write_key(&previous, &old)?;
    }
    let (pkcs8, public) = up::keygen()?;
    write_key(&key, &pkcs8)?;
    keys.push(public);
    Ok((key, keys))
}

/// Make the signing key for app `id` in `dir`.
pub fn keygen(dir: &Path, id: &str, rotate: bool) -> Result<String, String> {
    let (key, keys) = new_key(dir, id, rotate)?;
    let list = keys.iter().map(|k| format!("\"{k}\"")).collect::<Vec<_>>().join(", ");
    Ok(format!(
        "signing key: {} (back it up somewhere safe; never commit it; $XDG_CONFIG_HOME moves its folder)\n\
         put this in the app's aio.toml:\n\n[updates]\nurl = \"https://…\"  # or \"github:<owner>/<repo>\": its GitHub releases\n\
         keys = [{list}]\n\nand the url's host in the network list already there (the app fetches its updates from it):\n\n\
         [capabilities]\nnetwork = [\"<the url's host>\"]  # github: \"github.com\", \"*.githubusercontent.com\"",
        key.display()
    ))
}

/// `ax keygen --aio`: aio-rs's own update key, which signs the `SHA256SUMS`
/// of every `ax release --aio` from then on.
pub fn keygen_aio(dir: &Path, rotate: bool) -> Result<String, String> {
    let (key, keys) = new_key(dir, AIO_KEY_ID, rotate)?;
    Ok(format!(
        "aio-rs's update key: {} (back it up somewhere safe; never commit it)\n\
         ax release --aio signs SHA256SUMS with it from now on; publish its public key with the releases:\n  {}",
        key.display(),
        keys.join("\n  ")
    ))
}

/// What a build reads: it runs once in a throwaway data dir and says.
fn contract_of(binary: &Path) -> Result<up::Contract, String> {
    aio_shell::update::contract_of(binary)
}

/// The patch that makes `new` (a bundle for `os`) from `old`, unpacked in
/// `scratch` (removed after).
fn patch(
    unpack: &dyn Fn(&[u8], &Path) -> std::io::Result<()>,
    old: &[u8],
    new: &[u8],
    scratch: &Path,
) -> Result<Vec<u8>, String> {
    let (o, n) = (scratch.join("old"), scratch.join("new"));
    let made = (|| {
        for (dir, bundle) in [(&o, old), (&n, new)] {
            std::fs::create_dir_all(dir)?;
            unpack(bundle, dir)?;
        }
        aio_shell::delta::make(&o, &n)
    })();
    _ = std::fs::remove_dir_all(scratch);
    made.map_err(|e| e.to_string())
}

/// A bundle laid out as it is installed: a `.tar.gz` or zip unpacked, an
/// AppImage as the one file it is.
type Unpack<'a> = dyn Fn(&[u8], &Path) -> std::io::Result<()> + 'a;
/// A version's bundle file name.
type Named<'a> = dyn Fn(&str) -> String + 'a;

/// The patches to `m` (bundle `new`) from each of `from` (versions whose
/// bundles are in `out`, named as `bundle`'s), written beside it: their
/// list, signed, and the files to upload. One over half the release is
/// left out, said: no app would use it.
fn patches(
    out: &Path,
    m: &up::Manifest,
    new: &[u8],
    from: &[&str],
    (named, bundle, unpack): (&str, &Named<'_>, &Unpack<'_>),
    key: &[u8],
) -> Result<Vec<PathBuf>, String> {
    let r = &m.release;
    let json = out.join(format!("{named}.patches.json"));
    let (mut list, mut files) = (vec![], vec![]);
    for v in from {
        let old_file = out.join(bundle(v));
        let old = std::fs::read(&old_file).map_err(|e| format!("--patch-from {v}: {}: {e}", old_file.display()))?;
        let bytes = patch(unpack, &old, new, &out.join(format!(".patch-{v}")))
            .map_err(|e| format!("--patch-from {v}: no patch made: {e}"))?;
        if bytes.len() as u64 * 2 > r.size {
            eprintln!("ax release: the patch from {v} is {} bytes, over half the release: left out", bytes.len());
            continue;
        }
        let file = format!("{}-{}-from-{v}.patch", r.id, r.version);
        std::fs::write(out.join(&file), &bytes).map_err(|e| e.to_string())?;
        list.push(up::Patch {
            from: v.to_string(),
            size: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
            file: file.clone(),
        });
        files.push(out.join(file));
    }
    if list.is_empty() {
        // An earlier release's list here would be uploaded with this one.
        _ = std::fs::remove_file(&json);
        return Ok(files);
    }
    let signed = up::sign_patches(m, list, key)?;
    let text = aio::serde_json::to_string_pretty(&signed).map_err(|e| e.to_string())?;
    aio::paths::write_atomic(&json, format!("{text}\n").as_bytes(), false).map_err(|e| e.to_string())?;
    files.insert(0, json);
    Ok(files)
}

/// What `ax release` is asked besides the app.
#[derive(Default)]
pub struct Ask<'a> {
    pub channel: Option<&'a str>,
    pub notes: &'a str,
    /// The platforms to release for (`--platforms`, `--windows`); none: this machine's.
    pub platforms: Vec<&'static crate::platform::Platform>,
    /// Versions to make patches from (`--patch-from`).
    pub patch_from: Vec<&'a str>,
    /// The oldest version it installs over (`--min-from`).
    pub min_from: Option<&'a str>,
    /// Sign in the commit it was built from (`--commit`, `[updates] sign_commit`).
    pub commit: bool,
    /// Where the channel dirs go (`--dir`); none: the app's `dist/`.
    pub dir: Option<&'a Path>,
    /// What the release reads (`--data contract.json`); none: asked of the build.
    pub data: Option<&'a Path>,
    /// Release the bundles `ax build` left in `dist/` (`--no-build`).
    pub no_build: bool,
    /// Release a dirty or no-git build too (`--allow-dirty`), and say so.
    pub allow_dirty: bool,
    /// Release the Linux AppImage (`--appimage`): signed as
    /// `<os>-<arch>.appimage.json`, what an app run from one updates from.
    pub appimage: bool,
    /// The signing key's file (`--key`); none: `<keys dir>/<id>.key`.
    pub key: Option<&'a Path>,
    /// Signed with a key made for it alone (`--unsigned`): a local look at a
    /// release, with no key kept and no `[updates] keys`; no app takes it.
    pub unsigned: bool,
}

/// Build, sign and lay out a release of `name` for each platform asked.
pub fn run(name: &str, ask: &Ask) -> Result<String, String> {
    let platforms: Vec<_> = match ask.platforms.is_empty() {
        true => vec![None],
        false => ask.platforms.iter().map(|p| Some(*p)).collect(),
    };
    Ok(platforms.into_iter().map(|p| one(name, p, ask)).collect::<Result<Vec<_>, _>>()?.join("\n"))
}

/// The contract a `--data` file gives.
fn contract_in(file: &Path) -> Result<up::Contract, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("--data {}: {e}", file.display()))?;
    aio::serde_json::from_slice(&bytes).map_err(|e| format!("--data {}: not a data contract ({e})", file.display()))
}

/// The release of `name` for platform `p` (None: this machine's).
fn one(name: &str, p: Option<&'static crate::platform::Platform>, ask: &Ask) -> Result<String, String> {
    let (b, bundle) = match ask.no_build {
        true => crate::build::built(name, p)?,
        false => crate::build::make(name, p, ask.channel)?,
    };
    let (bundle, manifest) = match ask.appimage {
        false => (bundle, format!("{}-{}", b.platform.os, b.platform.arch)),
        true => (appimage_of(&b, &bundle, ask)?, format!("{}-{}.appimage", b.platform.os, b.platform.arch)),
    };
    if let Some(why) = crate::version::unreleasable(&b.app.version) {
        match ask.allow_dirty {
            true => eprintln!("ax release: {why}: released anyway (--allow-dirty)"),
            false => return Err(why),
        }
    }
    let id = &b.app.id;
    let u = &b.cfg.updates;
    let channel = ask.channel.unwrap_or(&u.channel);
    // A build follows the channel it was made for: `ax build`'s, aio.toml's.
    if ask.no_build && channel != u.channel {
        return Err(format!(
            "--no-build: dist/ holds a build for channel {}; one for {channel} is built for it (leave out --no-build)",
            u.channel
        ));
    }
    let (key, signers) = signing_keys(id, name, u, ask)?;
    // What it reads is the same on every OS: a cross-built bundle is asked
    // through this machine's build of it.
    let data = match ask.data {
        Some(f) => contract_in(f)?,
        None if b.platform.os == std::env::consts::OS => contract_of(&b.binary)?,
        None if ask.no_build => {
            return Err(format!(
                "--no-build: the {} build can't say here what data it reads: give it with --data <contract.json>",
                b.platform.name
            ));
        }
        None => {
            let meta = crate::dev::metadata()?;
            let pkg = crate::dev::locate(&meta, name)?;
            contract_of(&crate::build::release_binary(
                &meta,
                &pkg.name,
                name,
                None,
                None,
                Some(channel),
                &b.app.version,
                &crate::build::shell_features(&meta, &pkg.name, &b.cfg, true),
                &Default::default(),
            )?)?
        }
    };
    let out = ask.dir.unwrap_or(&b.dist).join(channel);
    let (all, manifest) = lay_out(
        &b,
        (&bundle, &manifest),
        &out,
        (channel, ask.notes, data, ask.min_from, ask.commit),
        (&key, &signers),
        &ask.patch_from,
    )?;
    // Built for this release: it lives in the channel dir now.
    if !ask.no_build {
        _ = std::fs::remove_file(&bundle);
    }
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    Ok(format!(
        "{id} {} on {channel}, {}:\n  {}\n{}",
        manifest.release.version,
        signed(ask.unsigned),
        all.join("\n  "),
        publish_help(u.url.as_deref(), channel, &b.app.version, &refs)
    ))
}

/// `--appimage`: build `b`'s AppImage in place of its `bundle` (removed,
/// made for this release), or with `--no-build` the one `ax build
/// --appimage` left in `dist/`.
fn appimage_of(b: &crate::build::Built, bundle: &Path, ask: &Ask) -> Result<PathBuf, String> {
    if b.platform.os != "linux" {
        return Err(format!("--appimage: an AppImage is a Linux package, not one for {}", b.platform.name));
    }
    if ask.no_build {
        let image = b.dist.join(format!("{}-{}-{}.AppImage", b.app.id, b.app.version, b.platform.arch));
        return match image.is_file() {
            true => Ok(image),
            false => Err(format!("{}: not built (ax build {} --appimage)", image.display(), b.app.id)),
        };
    }
    _ = std::fs::remove_file(bundle);
    #[cfg(unix)]
    return crate::package::appimage(b);
    #[cfg(not(unix))]
    Err("--appimage: an AppImage is built on Linux".into())
}

/// Sign build `b`'s `bundle` and lay a copy out in `out` beside its
/// manifest (`<manifest>.json`), and the patches from `patch_from`; the
/// files, and the manifest.
fn lay_out(
    b: &crate::build::Built,
    (bundle, named): (&Path, &str),
    out: &Path,
    (channel, notes, data, min, commit): (&str, &str, up::Contract, Option<&str>, bool),
    (key, signers): (&[u8], &[Vec<u8>]),
    patch_from: &[&str],
) -> Result<(Vec<String>, up::Manifest), String> {
    let min_from = min_from(min, &b.app.version)?;
    let bytes = std::fs::read(bundle).map_err(|e| format!("{}: {e}", bundle.display()))?;
    let json = out.join(format!("{named}.json"));
    if let Some(why) = rebuilt(std::fs::read_to_string(&json).ok().as_deref(), &b.app.version, &sha256_hex(&bytes)) {
        return Err(why);
    }
    let file = bundle.file_name().and_then(|f| f.to_str()).ok_or("bad bundle name")?.to_string();
    let release = up::Release {
        format: up::FORMAT,
        id: b.app.id.clone(),
        version: b.app.version.clone(),
        channel: channel.to_string(),
        os: b.platform.os.into(),
        arch: b.platform.arch.into(),
        file: file.clone(),
        sha256: sha256_hex(&bytes),
        size: bytes.len() as u64,
        released_at: now(),
        notes: notes.to_string(),
        data,
        roster: Some(up::roster(&b.cfg.updates.keys, signers)?),
        min_from,
        commit: signed_commit(commit || b.cfg.updates.sign_commit, &b.dist, &b.app.id)?,
    };
    if let Some(w) = unread_before_0_3(&release) {
        eprintln!("{w}");
    }
    let manifest = up::sign(release, key)?;
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    std::fs::copy(bundle, out.join(&file)).map_err(|e| format!("{}: {e}", out.join(&file).display()))?;
    let text = aio::serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    aio::paths::write_atomic(&json, format!("{text}\n").as_bytes(), false).map_err(|e| e.to_string())?;
    // An AppImage is one file: its patch is a one-file tree (zstd against the image here).
    let image = named.ends_with(".appimage");
    let name_of = |v: &str| match image {
        true => format!("{}-{v}-{}.AppImage", b.app.id, b.platform.arch),
        false => {
            let old =
                crate::build::App { id: b.app.id.clone(), name: String::new(), version: v.to_string(), icon: None };
            crate::build::bundle_file(&old, b.platform)
        }
    };
    let unpack = |bundle: &[u8], into: &Path| match image {
        true => aio_shell::update::unpack_appimage(&b.app.id, bundle, into),
        false => aio_shell::update::unpack_for(b.platform.os, bundle, into),
    };
    let extra = patches(out, &manifest, &bytes, patch_from, (named, &name_of, &unpack), key)?;
    let mut all = vec![json.display().to_string(), out.join(&file).display().to_string()];
    all.extend(extra.iter().map(|p| p.display().to_string()));
    Ok((all, manifest))
}

/// Why a release of `version` (its bundle's `sha256`) can't go over the
/// manifest `old` laid out there: the same version with other bytes (aio's
/// "same version, new build"). An install that has that version is never
/// offered another build of it, so a new build is a new version: a commit
/// raises the build number. The same bytes again (a rebuild that matched)
/// go over it (pure).
fn rebuilt(old: Option<&str>, version: &str, sha256: &str) -> Option<String> {
    let old: up::Manifest = aio::serde_json::from_str(old?).ok()?;
    (old.release.version == version && old.release.sha256 != sha256).then(|| {
        format!(
            "{version} is released here already with other bytes: an install that has {version} would never be offered \
             these. A new build is a new version: commit (git commit --allow-empty -m rebuild raises the build number), \
             then release again"
        )
    })
}

/// `--min-from`, checked: a version, at most the release's own (pure).
fn min_from(min: Option<&str>, version: &str) -> Result<Option<String>, String> {
    let Some(m) = min else { return Ok(None) };
    match up::compare(m, version) {
        None => Err(format!("--min-from {m}: not a version (write it as 1.2.0)")),
        Some(std::cmp::Ordering::Greater) => {
            Err(format!("--min-from {m} is above this release's version {version}: no install could take it"))
        }
        _ => Ok(Some(m.to_string())),
    }
}

/// A warning when release `r` names what an install on aio-rs older than
/// 0.3 can't read (its commit, a minFrom): such an install refuses the
/// whole manifest, so it needs a plain release to update through (pure).
fn unread_before_0_3(r: &up::Release) -> Option<String> {
    let named: Vec<&str> = [(r.commit.is_some(), "its commit"), (r.min_from.is_some(), "a minFrom")]
        .into_iter()
        .filter_map(|(on, what)| on.then_some(what))
        .collect();
    (!named.is_empty()).then(|| {
        format!(
            "warning: this release names {}: an install on aio-rs older than 0.3 can't read it and never updates; \
             publish a plain release (no --commit, --min-from or [updates] sign_commit) built on aio-rs 0.3 first, \
             for those installs to update through",
            named.join(" and ")
        )
    })
}

/// The commit app `id`'s build in `dist` was made from (its build record),
/// when `asked`: only installs on aio-rs 0.3 or newer read a manifest that
/// names it, so a plain release names none.
fn signed_commit(asked: bool, dist: &Path, id: &str) -> Result<Option<String>, String> {
    if !asked {
        return Ok(None);
    }
    let rec: Option<aio::serde_json::Value> =
        std::fs::read(crate::manifest::path(dist, id)).ok().and_then(|b| aio::serde_json::from_slice(&b).ok());
    match rec.as_ref().and_then(|r| r["commit"].as_str()) {
        Some(c) => Ok(Some(c.to_string())),
        None => Err(format!("--commit: the build of {id} in {} names no commit (built outside git?)", dist.display())),
    }
}

/// App `id`'s signing key (it must be one `u` trusts), and every key that
/// signs its roster: it and the one it replaced, if kept.
/// How a release is signed, as said after it (pure).
fn signed(unsigned: bool) -> &'static str {
    match unsigned {
        true => "UNSIGNED (--unsigned: a key no app trusts; to look at, never to publish)",
        false => "signed",
    }
}

/// The key that signs `id`'s release, and every key that signs it (the
/// previous one too, through a rotation). `unsigned`: a key made for this
/// release alone, which no app trusts (a local look at a release).
fn signing_keys(id: &str, name: &str, u: &aio::config::Updates, ask: &Ask) -> Result<(Vec<u8>, Vec<Vec<u8>>), String> {
    if ask.unsigned {
        let (key, _) = up::keygen()?;
        return Ok((key.clone(), vec![key]));
    }
    // `--key <file>`: that key, its previous one beside it (`<name>.previous.key`).
    let file = ask.key.map_or_else(|| keys_dir().join(format!("{id}.key")), Path::to_path_buf);
    let key = read_key(&file).map_err(|e| match ask.key {
        Some(_) => e,
        None => format!(
            "{e} (ax keygen {name} makes it, in $XDG_CONFIG_HOME/aio-rt/keys; --key: another file; --unsigned: a local release without)"
        ),
    })?;
    if !u.keys.contains(&up::public_key(&key)?) {
        return Err("aio.toml's [updates] keys don't include this signing key: the app would refuse the release".into());
    }
    let mut signers = vec![key.clone()];
    let stem = file.file_stem().map_or_else(Default::default, |s| s.to_string_lossy().into_owned());
    if let Ok(prev) = read_key(&file.with_file_name(format!("{stem}.previous.key"))) {
        signers.push(prev);
    }
    Ok((key, signers))
}

fn now() -> String {
    aio::log::stamp(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64),
    )
}

/// The CPU an APK named `<id>-<version>-android-<abi>.apk` is for, as
/// manifests name it (`std::env::consts::ARCH` on the device).
fn apk_arch(file: &str) -> Option<&'static str> {
    let abi = file.strip_suffix(".apk")?.rsplit_once("-android-")?.1;
    let target = crate::android::ABIS.iter().find(|(a, _)| *a == abi)?.1;
    // armv7 runs as `arm` (Rust's ARCH there).
    Some(if target.starts_with("armv7") { "arm" } else { target.split('-').next().unwrap_or(target) })
}

/// Sign each APK of `apks` as `base` for its CPU and lay both out in `out`:
/// `android-<arch>.json` beside the APK, moved there (`keep`: copied, so
/// `dist/` still holds it). An app bundle (`.aab`, for Play) and the
/// universal APK (one to share) are no update: left where they are.
fn android_releases(
    out: &Path,
    apks: &[PathBuf],
    base: &up::Release,
    key: &[u8],
    keep: bool,
) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let mut files = vec![];
    for apk in apks {
        let file = apk.file_name().and_then(|f| f.to_str()).ok_or("bad APK name")?;
        if file.ends_with(".aab") || file.ends_with("-android-universal.apk") {
            continue;
        }
        let arch = apk_arch(file).ok_or_else(|| format!("{file}: not an APK of a known CPU type"))?;
        let bytes = std::fs::read(apk).map_err(|e| format!("{}: {e}", apk.display()))?;
        let release = up::Release {
            os: "android".into(),
            arch: arch.into(),
            file: file.into(),
            sha256: sha256_hex(&bytes),
            size: bytes.len() as u64,
            ..base.clone()
        };
        let text = aio::serde_json::to_string_pretty(&up::sign(release, key)?).map_err(|e| e.to_string())?;
        let json = out.join(format!("android-{arch}.json"));
        if apk != &out.join(file) {
            match keep {
                true => std::fs::copy(apk, out.join(file)).map(drop),
                false => std::fs::rename(apk, out.join(file)),
            }
            .map_err(|e| e.to_string())?;
        }
        aio::paths::write_atomic(&json, format!("{text}\n").as_bytes(), false).map_err(|e| e.to_string())?;
        files.extend([json, out.join(file)]);
    }
    Ok(files)
}

/// The APKs `ax build` left in `dist` for `id` at `version`, one per CPU
/// type: refused when one is missing.
fn apks_built(dist: &Path, id: &str, version: &str, name: &str) -> Result<Vec<PathBuf>, String> {
    let apks: Vec<PathBuf> =
        crate::android::ABIS.iter().map(|(abi, _)| dist.join(format!("{id}-{version}-android-{abi}.apk"))).collect();
    match apks.iter().find(|a| !a.is_file()) {
        Some(a) => {
            Err(format!("{}: not built (ax build {name} --targets android), or leave out --no-build", a.display()))
        }
        None => Ok(apks),
    }
}

/// `ax release <app> --android`: its APKs, one per CPU type, signed for
/// `channel` (the one in `aio.toml`: an APK carries no other). An Android
/// app can't replace its own APK: it offers the release as one to open
/// from where it is published, and the system installer puts it in place.
/// `--dir`, `--data` and `--no-build` as for the desktop.
pub fn run_android(name: &str, ask: &Ask) -> Result<String, String> {
    let meta = crate::dev::metadata()?;
    let pkg = crate::dev::locate(&meta, name)?;
    let (app, cfg) = crate::build::app_of(&pkg)?;
    if let Some(why) = crate::version::unreleasable(&app.version) {
        match ask.allow_dirty {
            true => eprintln!("ax release: {why}: released anyway (--allow-dirty)"),
            false => return Err(why),
        }
    }
    let u = &cfg.updates;
    if ask.channel.is_some_and(|c| c != u.channel) {
        return Err(format!("an APK follows aio.toml's channel ({}): set [updates] channel there instead", u.channel));
    }
    let (key, signers) = signing_keys(&app.id, name, u, ask)?;
    let target = Path::new(meta["target_directory"].as_str().ok_or("cargo metadata: no target_directory")?);
    let dist = crate::build::dist(
        Path::new(meta["workspace_root"].as_str().ok_or("cargo metadata: no workspace_root")?),
        &cfg,
    );
    // What it reads is the same everywhere: asked through this machine's build.
    let data = match ask.data {
        Some(f) => contract_in(f)?,
        None if ask.no_build => match crate::bundle::binary(target, None, name, std::env::consts::OS) {
            probe if probe.is_file() => contract_of(&probe)?,
            _ => {
                return Err(format!(
                    "--no-build: no desktop build of {name} here says what data the APK reads: give it with --data <contract.json>"
                ));
            }
        },
        None => contract_of(
            &crate::build::release_binary(
                &meta,
                &pkg.name,
                name,
                None,
                None,
                None,
                &app.version,
                &crate::build::shell_features(&meta, &pkg.name, &cfg, true),
                &Default::default(),
            )
            .map_err(|e| format!("{e}: the desktop build of {name} says which data the APK reads"))?,
        )?,
    };
    let apks = match ask.no_build {
        true => apks_built(&dist, &app.id, &app.version, name)?,
        false => crate::build::android(name, crate::android::Ask::RELEASE)?.1,
    };
    let base = up::Release {
        format: up::FORMAT,
        id: app.id.clone(),
        version: app.version.clone(),
        channel: u.channel.clone(),
        os: String::new(),
        arch: String::new(),
        file: String::new(),
        sha256: String::new(),
        size: 0,
        released_at: now(),
        notes: ask.notes.to_string(),
        data,
        roster: Some(up::roster(&u.keys, &signers)?),
        min_from: min_from(ask.min_from, &app.version)?,
        commit: signed_commit(ask.commit || u.sign_commit, &dist, &app.id)?,
    };
    if let Some(w) = unread_before_0_3(&base) {
        eprintln!("{w}");
    }
    let out = ask.dir.unwrap_or(&dist).join(&u.channel);
    let files = android_releases(&out, &apks, &base, &key, ask.no_build)?;
    let all: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    Ok(format!(
        "{} {} for Android on {}, {}:\n  {}\n{}",
        app.id,
        app.version,
        u.channel,
        signed(ask.unsigned),
        all.join("\n  "),
        publish_help(u.url.as_deref(), &u.channel, &app.version, &refs)
    ))
}

/// Where a release's files go so apps find them (`[updates] url`).
fn publish_help(url: Option<&str>, channel: &str, version: &str, files: &[&str]) -> String {
    let files = files.join(" ");
    match url {
        None => format!("set [updates] url in aio.toml, then upload them to <url>/{channel}/"),
        Some(u) => match u.strip_prefix("github:").and_then(up::github_repo) {
            Some((owner, repo)) if channel == "stable" => format!(
                "attach them to a new GitHub release (apps read the latest one):\n  gh release create v{version} --repo {owner}/{repo} {files}"
            ),
            Some((owner, repo)) => format!(
                "attach them to the GitHub release tagged {channel}, replacing the files there:\n  gh release upload {channel} --repo {owner}/{repo} --clobber {files}"
            ),
            None => format!("upload them to {}/", up::release_dir(u, channel)),
        },
    }
}

fn sha256_hex(b: &[u8]) -> String {
    aio::sha256::hex(&aio::sha256::Sha256::digest(b))
}

/// Where a workflow gets the `ax` command: the git source (and locked
/// commit) the app's `aio-rs` comes from, from its `cargo metadata` entry.
fn aio_source(pkg: &aio::serde_json::Value) -> Option<(String, String)> {
    let dep = pkg["dependencies"].as_array()?.iter().find(|d| d["name"] == "aio-rs")?;
    let src = dep["source"].as_str()?.strip_prefix("git+")?;
    let (url, commit) = src.split_once('#')?;
    Some((url.split('?').next()?.to_string(), commit.to_string()))
}

/// The release workflow for app `id` (binary `bin`) on `channel`: on a `v*`
/// tag matching the app's version, each OS builds and signs its release
/// (`ax release`, the key from the `AIO_SIGNING_KEY` secret); then one job
/// puts every file on the GitHub release, files before manifests, as a draft
/// until all are there. `aio`: the git source and commit the app's aio-rs
/// comes from (`ax` is installed from it, as `ax create` writes); none, a path dependency (`ax
/// create --aio`): aio-rs is checked out beside the app at this `ax`'s
/// release tag (repository variables `AIO_REPO`, `AIO_REF` name another),
/// the app's aio-rs paths pointed there, and `ax` installed from it.
/// `pages` (aio's `ship github`): the channel's files also deployed to
/// GitHub Pages, under `<channel>/`, for `[updates] url =
/// "https://<owner>.github.io/<repo>"`; `web`: the app's web build at the
/// site's root beside them.
pub fn workflow(id: &str, bin: &str, channel: &str, aio: Option<(&str, &str)>, (pages, web): (bool, bool)) -> String {
    let get_aio = match aio {
        Some((src, rev)) => format!(
            "      - name: Install ax\n        run: cargo install --locked --git \"{src}\" --rev \"{rev}\" aio-rs-cli\n"
        ),
        None => format!(
            r#"      # The app's aio-rs is a path dependency (`ax create --aio`): aio-rs beside
      # the app, its paths pointed there.
      - uses: actions/checkout@v4
        with:
          repository: ${{{{ vars.AIO_REPO || 'riagentic/aio-rs' }}}}
          ref: ${{{{ vars.AIO_REF || 'v{v}' }}}}
          path: aio-rs
      - name: Point the app's aio-rs paths at that checkout
        run: |
          find app -name Cargo.toml -not -path '*/target/*' -exec perl -pi -e '{REPOINT}' {{}} +
      - name: Install ax
        working-directory: aio-rs
        run: cargo install --locked --path crates/aio-cli
"#,
            v = env!("CARGO_PKG_VERSION")
        ),
    };
    let (tag, make, latest) = if channel == "stable" {
        (
            "\"$GITHUB_REF_NAME\"".to_string(),
            "gh release create \"$GITHUB_REF_NAME\" --draft --verify-tag --generate-notes".to_string(),
            "\n            # Latest (what apps on stable read) only for the highest version.\
             \n            newest=$(git tag -l 'v*' | { grep -v -- - || true; } | sort -V | tail -n 1)\
             \n            latest=false; [ \"$GITHUB_REF_NAME\" = \"$newest\" ] && latest=true\
             \n            gh release edit \"$GITHUB_REF_NAME\" --draft=false --latest=\"$latest\""
                .to_string(),
        )
    } else {
        (
            channel.to_string(),
            format!(
                "gh release create {channel} --draft --prerelease --target \"$GITHUB_SHA\" --title \"{channel} channel\" \
                 --notes \"The newest {channel} build: {id} on updates channel {channel} reads this release.\""
            ),
            format!("\n            gh release edit {channel} --draft=false"),
        )
    };
    let web_job = match web {
        false => String::new(),
        true => format!(
            r#"
  # The app's web build, for the root of the Pages site.
  web:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
        with:
          path: app
          fetch-depth: 0
      - run: rustup toolchain install && rustup target add wasm32-unknown-unknown
        working-directory: app
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: app
{get_aio}      - name: Build it for the web
        working-directory: app
        run: ax build {bin} --web
      - uses: actions/upload-artifact@v4
        with:
          name: web
          path: app/dist/{id}-web
          if-no-files-found: error
"#
        ),
    };
    let (needs, get_web) = match web {
        false => ("build", String::new()),
        true => (
            "[build, web]",
            "      - uses: actions/download-artifact@v4
        with:
          name: web
          path: site
"
            .into(),
        ),
    };
    let pages_job = match pages {
        false => String::new(),
        true => format!(
            r#"      # The Pages site: what an app on url = "https://<owner>.github.io/<repo>"
      # reads, the channel's files under {channel}/. A deploy replaces the
      # whole site: one channel per site. Pages takes files up to 100 MB.
      - name: Lay out the Pages site
        run: mkdir -p site/{channel} && cp out/* site/{channel}/
      - uses: actions/upload-pages-artifact@v3
        with:
          path: site

  pages:
    needs: publish
    runs-on: ubuntu-24.04
    permissions:
      pages: write
      id-token: write
    # One deploy at a time: two racing would leave half of each.
    concurrency:
      group: pages
      cancel-in-progress: false
    environment:
      name: github-pages
      url: ${{{{ steps.deploy.outputs.page_url }}}}
    steps:
      - id: deploy
        uses: actions/deploy-pages@v4
"#
        ),
    };
    format!(
        r#"# Made by `ax release {bin} --workflow`. Edit freely: it is yours now.
#
# On a v* tag (v<version of {bin}>): each OS builds, signs and lays out its
# release of {id} on channel {channel} (`ax release`), then the files go on
# the GitHub release {tag}: a draft until every file is on it, the manifests
# last, so no app reads a manifest whose bundle isn't up yet. Apps on
# `[updates] url = "github:<owner>/<repo>"` find it there.
#
# Signing: put the text of ~/.config/aio-rt/keys/{id}.key in the repository
# secret AIO_SIGNING_KEY. Without it the workflow fails: an app refuses an
# unsigned release.
name: release-{bin}

on:
  push:
    tags: ["v*"]
  workflow_dispatch:

jobs:
  build:
    strategy:
      matrix:
        include:
          - {{ os: ubuntu-24.04, flags: "" }}
          - {{ os: ubuntu-24.04, flags: "--windows" }}
          - {{ os: macos-15, flags: "" }}
    runs-on: ${{{{ matrix.os }}}}
    steps:
      - name: Publish only from a v* tag
        run: |
          case "$GITHUB_REF" in
            refs/tags/v*) ;;
            *) echo "::error::$GITHUB_REF is not a v* tag: tag the release (git tag v1.2.3 && git push origin v1.2.3)"; exit 1 ;;
          esac
      - uses: actions/checkout@v4
        with:
          path: app
          fetch-depth: 0
      - if: matrix.flags == '--windows'
        run: sudo apt-get update && sudo apt-get install -y --no-install-recommends gcc-mingw-w64-x86-64
      - run: rustup toolchain install && rustup target add x86_64-pc-windows-gnu
        working-directory: app
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: app
{get_aio}      - name: The signing key
        env:
          AIO_SIGNING_KEY: ${{{{ secrets.AIO_SIGNING_KEY }}}}
        run: |
          test -n "$AIO_SIGNING_KEY" || {{ echo "::error::no AIO_SIGNING_KEY secret: an app refuses an unsigned release. Put the text of ~/.config/aio-rt/keys/{id}.key there."; exit 1; }}
          mkdir -p "$HOME/.config/aio-rt/keys" && chmod 700 "$HOME/.config/aio-rt/keys"
          printf '%s' "$AIO_SIGNING_KEY" > "$HOME/.config/aio-rt/keys/{id}.key" && chmod 600 "$HOME/.config/aio-rt/keys/{id}.key"
      - name: Build, sign and lay out the release
        working-directory: app
        env:
          FLAGS: ${{{{ matrix.flags }}}}
        run: |
          set -euo pipefail
          ax release {bin} --channel {channel} $FLAGS
          rm -f "$HOME/.config/aio-rt/keys/{id}.key"
          # The tag is the version: a release of another one would be offered as this.
          grep -q "\"version\": \"${{GITHUB_REF_NAME#v}}\"" dist/{channel}/*-*.json \
            || {{ echo "::error::$GITHUB_REF_NAME is not the version of {bin} (aio.toml's [app] version, else its Cargo.toml)"; exit 1; }}
      - uses: actions/upload-artifact@v4
        with:
          name: {id}-${{{{ strategy.job-index }}}}
          path: app/dist/{channel}/*
          if-no-files-found: error
{web_job}
  publish:
    needs: {needs}
    runs-on: ubuntu-24.04
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: actions/download-artifact@v4
        with:
          pattern: {id}-[0-9]*
          path: out
          merge-multiple: true
{get_web}      - name: Put the files on the release, then publish it
        env:
          GH_TOKEN: ${{{{ github.token }}}}
        run: |
          set -euo pipefail
          made=
          if ! gh release view {tag} >/dev/null 2>&1; then
            {make}
            made=1
          fi
          # Files first, manifests last.
          find out -maxdepth 1 -type f ! -name '*.json' -print0 | xargs -0 -r gh release upload {tag} --clobber
          gh release upload {tag} out/*.json --clobber
          if [ -n "$made" ]; then{latest}
          fi
{pages_job}"#
    )
}

/// Perl's edit of an app's `Cargo.toml` in the workflow: each aio-rs crate's
/// `path` (`ax create --aio`'s checkout) to `$GITHUB_WORKSPACE/aio-rs/crates/<crate>`.
const REPOINT: &str =
    r##"s#(package = "aio-rs[a-z-]*", path = ")[^"]*/(aio[a-z-]*)"#$1$ENV{GITHUB_WORKSPACE}/aio-rs/crates/$2"#"##;

/// `ax release <app> --workflow`: the workflow in the workspace's
/// `.github/workflows/release-<app>.yml`, never over one there.
pub fn write_workflow(name: &str, channel: Option<&str>, (pages, web): (bool, bool)) -> Result<String, String> {
    if web && !pages {
        return Err("--web puts the web build on the Pages site: with --pages".into());
    }
    let meta = crate::dev::metadata()?;
    let pkg = crate::dev::locate(&meta, name)?;
    let (app, cfg) = crate::build::app_of(&pkg)?;
    let channel = channel.unwrap_or(&cfg.updates.channel);
    let pkgs = meta["packages"].as_array().ok_or("cargo metadata: no packages")?;
    let source = pkgs.iter().find(|p| p["name"] == pkg.name).and_then(aio_source);
    let root = meta["workspace_root"].as_str().ok_or("cargo metadata: no workspace_root")?;
    let file = Path::new(root).join(".github/workflows").join(format!("release-{name}.yml"));
    if file.exists() {
        return Err(format!("{} exists: it is yours now (remove it to make it again)", file.display()));
    }
    std::fs::create_dir_all(file.parent().ok_or("bad path")?).map_err(|e| e.to_string())?;
    let text = workflow(&app.id, name, channel, source.as_ref().map(|(g, r)| (g.as_str(), r.as_str())), (pages, web));
    std::fs::write(&file, text).map_err(|e| e.to_string())?;
    let from = match &source {
        Some((g, r)) => format!("ax is installed from {g} at {r}"),
        None => format!(
            "aio-rs is a path dependency (ax create --aio): the workflow checks out riagentic/aio-rs at v{} beside the app \
             (repository variables AIO_REPO and AIO_REF name another)",
            env!("CARGO_PKG_VERSION")
        ),
    };
    let pages = match pages {
        true => format!(
            "\nGitHub Pages: Settings > Pages > Source: GitHub Actions; let a tag deploy (Settings > Environments > \
             github-pages > a tag rule v*); then in aio.toml [updates] url = \"https://<owner>.github.io/<repo>\" \
             (channel {channel} under it), its host in [capabilities] network"
        ),
        false => String::new(),
    };
    Ok(format!(
        "wrote {}\n{from}\nnext: put the text of {} in the repository secret AIO_SIGNING_KEY, then push a tag v{}{pages}",
        file.display(),
        keys_dir().join(format!("{}.key", app.id)).display(),
        app.version
    ))
}

/// The installers `ax release --aio` writes beside aio-rs's builds.
const INSTALL_SH: &str = include_str!("aio-install.sh");
const INSTALL_PS1: &str = include_str!("aio-install.ps1");

/// Where aio-rs `version`'s files are downloaded from: its GitHub release (pure).
pub fn aio_release_url(version: &str) -> String {
    format!("https://github.com/riagentic/aio-rs/releases/download/v{version}")
}

/// aio-rs's archive for `triple` (pure): a zip on Windows, else a tar.gz.
pub fn aio_archive(version: &str, triple: &str) -> String {
    let ext = if triple.contains("-windows-") { "zip" } else { "tar.gz" };
    format!("aio-rs-{version}-{triple}.{ext}")
}

/// The installers for `version`, each with the SHA-256 of its OS's builds
/// in `pins` (triple, sha) and the download `url` (empty: beside them) (pure).
pub fn installers(version: &str, url: &str, pins: &[(String, String)]) -> (String, String) {
    let (win, unix): (Vec<_>, Vec<_>) = pins.iter().partition(|(t, _)| t.contains("-windows-"));
    let sh = unix.iter().map(|(t, s)| format!("  {t}) want={s} ;;")).collect::<Vec<_>>().join("\n");
    let ps1 = win.iter().map(|(t, s)| format!("  '{t}' = '{s}'")).collect::<Vec<_>>().join("\n");
    let fill =
        |text: &str, pins: &str| text.replace("@VERSION@", version).replace("@URL@", url).replace("@PINS@", pins);
    (fill(INSTALL_SH, &sh), fill(INSTALL_PS1, &ps1))
}

/// The `SHA256SUMS` text for `files` (name, sha) (pure): `sha256sum -c` reads it.
pub fn sums(files: &[(String, String)]) -> String {
    files.iter().map(|(f, s)| format!("{s}  {f}\n")).collect()
}

/// Pack the binary `exe` as `out/<name>` (`ax`, `ax.exe` inside).
fn pack(exe: &Path, out: &Path) -> Result<(), String> {
    let inside = format!("ax{}", if out.extension().is_some_and(|e| e == "zip") { ".exe" } else { "" });
    if inside.ends_with(".exe") {
        let bytes = std::fs::read(exe).map_err(|e| format!("{}: {e}", exe.display()))?;
        let mut z = zip::ZipWriter::new(std::fs::File::create(out).map_err(|e| e.to_string())?);
        let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        z.start_file(inside, opts).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut z, &bytes).map_err(|e| e.to_string())?;
        return z.finish().map(drop).map_err(|e| e.to_string());
    }
    let stage = out.with_extension("stage");
    _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    // Runnable by anyone it is unpacked for, whatever this machine's umask made it.
    #[cfg(unix)]
    let copied = std::fs::copy(exe, stage.join(&inside)).and_then(|_| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(stage.join(&inside), std::fs::Permissions::from_mode(0o755))
    });
    #[cfg(not(unix))]
    let copied = std::fs::copy(exe, stage.join(&inside)).map(drop);
    let r = copied.map_err(|e| e.to_string()).and_then(|_| {
        let st = crate::build::tar_gz(out, &stage, &inside).status();
        match st {
            Ok(s) if s.success() => Ok(()),
            _ => Err(format!("tar could not make {}", out.display())),
        }
    });
    _ = std::fs::remove_dir_all(&stage);
    r
}

/// The environment that turns on each signature of aio-rs's own release; each
/// is skipped, said `unsigned: no keys configured`, until it is set.
pub const WINDOWS_CERT: &str = "AIO_WINDOWS_CERT";
pub const WINDOWS_CERT_PASSWORD: &str = "AIO_WINDOWS_CERT_PASSWORD";
pub const WINDOWS_TIMESTAMP: &str = "AIO_WINDOWS_TIMESTAMP_URL";
pub const MACOS_IDENTITY: &str = "AIO_MACOS_SIGN_IDENTITY";
pub const MACOS_NOTARY: &str = "AIO_MACOS_NOTARY_PROFILE";
pub const RELEASE_KEY: &str = "AIO_RELEASE_KEY";
/// aio-rs's update key in the keys dir (`ax keygen --aio`): no app id has a dot.
pub const AIO_KEY_ID: &str = "aio.release";

fn set(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

/// The line said for a signature not made (pure).
fn unsigned(what: &str, needs: &str) -> String {
    format!("  {what}: unsigned: no keys configured ({needs})")
}

/// Run a signing tool: its stdout, or why it failed.
fn tool(cmd: &mut std::process::Command, what: &str) -> Result<String, String> {
    let o = cmd.output().map_err(|e| format!("{what}: {e}"))?;
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    match o.status.success() {
        true => Ok(out),
        false => Err(format!("{what}: {} {}", out.trim(), String::from_utf8_lossy(&o.stderr).trim())),
    }
}

/// `exe` Authenticode-signed (osslsigncode) into `scratch` with the .pfx
/// `AIO_WINDOWS_CERT` names, else `exe` as it is; and the line said.
fn sign_windows(exe: &Path, scratch: &Path) -> Result<(PathBuf, String), String> {
    let what = "Windows Authenticode";
    let Some(cert) = set(WINDOWS_CERT) else {
        return Ok((exe.to_path_buf(), unsigned(what, &format!("{WINDOWS_CERT}, {WINDOWS_CERT_PASSWORD}"))));
    };
    let pass = set(WINDOWS_CERT_PASSWORD).ok_or(format!("{WINDOWS_CERT} is set, {WINDOWS_CERT_PASSWORD} is not"))?;
    // The password in a private file, never on a command line.
    let pass_file = scratch.join("cert.pass");
    aio::paths::write_atomic(&pass_file, pass.as_bytes(), true).map_err(|e| e.to_string())?;
    let (signed, ts) =
        (scratch.join("ax.exe"), set(WINDOWS_TIMESTAMP).unwrap_or("http://timestamp.digicert.com".into()));
    let r = tool(
        std::process::Command::new("osslsigncode")
            .args(["sign", "-pkcs12"])
            .arg(&cert)
            .arg("-readpass")
            .arg(&pass_file)
            .args(["-n", "aio-rs", "-ts", &ts, "-in"])
            .arg(exe)
            .arg("-out")
            .arg(&signed),
        "osslsigncode (Authenticode)",
    );
    _ = std::fs::remove_file(&pass_file);
    r?;
    Ok((signed, format!("  {what}: signed ({WINDOWS_CERT})")))
}

/// `exe` signed in place for Gatekeeper (`codesign`, hardened runtime) with
/// `AIO_MACOS_SIGN_IDENTITY`, then notarized with the `notarytool` keychain
/// profile `AIO_MACOS_NOTARY_PROFILE`; the line said.
fn sign_macos(exe: &Path, scratch: &Path) -> Result<String, String> {
    let what = "macOS codesign + notarize";
    let Some(id) = set(MACOS_IDENTITY) else {
        return Ok(unsigned(what, &format!("{MACOS_IDENTITY}, {MACOS_NOTARY}")));
    };
    if !cfg!(target_os = "macos") {
        return Err(format!("{MACOS_IDENTITY} is set, but codesign runs on a Mac: release aio-rs for macOS there"));
    }
    tool(
        std::process::Command::new("/usr/bin/codesign")
            .args(["--force", "--options", "runtime", "--timestamp", "--sign", &id])
            .arg(exe),
        "codesign",
    )?;
    let Some(profile) = set(MACOS_NOTARY) else {
        return Ok(format!("  {what}: signed ({MACOS_IDENTITY}); not notarized: {MACOS_NOTARY} is not set"));
    };
    let zip = scratch.join("ax.zip");
    tool(std::process::Command::new("/usr/bin/ditto").args(["-c", "-k"]).arg(exe).arg(&zip), "ditto")?;
    let said = tool(
        std::process::Command::new("/usr/bin/xcrun").args(["notarytool", "submit"]).arg(&zip).args([
            "--keychain-profile",
            &profile,
            "--wait",
        ]),
        "notarytool",
    )?;
    if !said.contains("status: Accepted") {
        return Err(format!("notarytool did not accept ax: {}", said.trim()));
    }
    Ok(format!("  {what}: signed and notarized"))
}

/// The key that signs aio-rs's `SHA256SUMS`: the file `AIO_RELEASE_KEY` names
/// (which must be there), else `aio.release.key` in `keys` when it is.
fn aio_update_key(keys: &Path) -> Result<Option<PathBuf>, String> {
    match set(RELEASE_KEY) {
        Some(f) if Path::new(&f).is_file() => Ok(Some(f.into())),
        Some(f) => Err(format!("{RELEASE_KEY}={f}: no key file there")),
        None => Ok(Some(keys.join(format!("{AIO_KEY_ID}.key"))).filter(|k| k.is_file())),
    }
}

/// `out/SHA256SUMS` signed with `key` as `out/SHA256SUMS.sig` (base64
/// Ed25519, `aio::update::verify`); none: no signature (an old one goes).
fn sign_sums(out: &Path, key: Option<&Path>) -> Result<String, String> {
    let (what, sig) = ("aio-rs update key (SHA256SUMS.sig)", out.join("SHA256SUMS.sig"));
    let Some(key) = key else {
        _ = std::fs::remove_file(&sig);
        return Ok(unsigned(what, &format!("{RELEASE_KEY}, or ax keygen --aio")));
    };
    let key = read_key(key)?;
    let sums = std::fs::read(out.join("SHA256SUMS")).map_err(|e| format!("SHA256SUMS: {e}"))?;
    std::fs::write(&sig, up::sign_with(&key, &sums)? + "\n").map_err(|e| format!("SHA256SUMS.sig: {e}"))?;
    Ok(format!("  {what}: signed by {}", up::public_key(&key)?))
}

/// aio-rs's archives of `version` named in `names` that this run did not
/// build (`built`): ones made on another machine (a Mac), as (triple, file) (pure).
fn built_elsewhere(names: &[String], version: &str, built: &[String]) -> Vec<(String, String)> {
    let prefix = format!("aio-rs-{version}-");
    let mut found: Vec<(String, String)> = names
        .iter()
        .filter_map(|n| {
            let t = n.strip_prefix(&prefix)?;
            let t = t.strip_suffix(".tar.gz").or_else(|| t.strip_suffix(".zip"))?;
            (aio_archive(version, t) == *n && !built.iter().any(|b| b == t)).then(|| (t.to_string(), n.clone()))
        })
        .collect();
    found.sort();
    found
}

/// Every file under `dir` but the sums and their signature, as (path from
/// `dir`, sha), sorted: what `SHA256SUMS` lists.
fn every_file(dir: &Path) -> Result<Vec<(String, String)>, String> {
    fn walk(root: &Path, d: &Path, into: &mut Vec<(String, String)>) -> Result<(), String> {
        for e in std::fs::read_dir(d).map_err(|e| format!("{}: {e}", d.display()))? {
            let p = e.map_err(|e| e.to_string())?.path();
            let rel = p.strip_prefix(root).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
            if rel.starts_with('.') || rel == "SHA256SUMS" || rel == "SHA256SUMS.sig" {
                continue;
            }
            if p.is_dir() {
                walk(root, &p, into)?;
            } else {
                into.push((rel, sha256_hex(&std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?)));
            }
        }
        Ok(())
    }
    let mut all = vec![];
    walk(dir, dir, &mut all)?;
    all.sort();
    Ok(all)
}

/// `ax release --aio`: aio-rs itself, prebuilt from this checkout for this
/// machine (and `targets`, e.g. Windows from Linux), each signed when its
/// keys are set and packed, in `dir` (default `dist/aio-rs/v<version>`).
/// aio-rs archives of this version already there (built on a Mac) are pinned
/// too: their SHA-256 in the two installers, and every file in `dir` in
/// `SHA256SUMS`, signed with aio-rs's update key when it exists. Read back
/// and checked before it is said done. `url`: where they will be downloaded
/// from (empty: the version's GitHub release, [`aio_release_url`]).
pub fn aio(targets: &[&str], dir: Option<&Path>, url: &str) -> Result<String, String> {
    let meta = crate::dev::metadata()?;
    let pkgs = meta["packages"].as_array().ok_or("cargo metadata: no packages")?;
    let cli =
        pkgs.iter().find(|p| p["name"] == "aio-rs-cli").ok_or("not an aio-rs checkout (no aio-rs-cli package here)")?;
    let version = cli["version"].as_str().ok_or("aio-rs-cli has no version")?;
    let root = Path::new(meta["workspace_root"].as_str().ok_or("cargo metadata: no workspace_root")?);
    let out = dir.map_or_else(|| root.join("dist/aio-rs").join(format!("v{version}")), Path::to_path_buf);
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let update_key = aio_update_key(&keys_dir())?;
    let (mut made, mut signing) = (vec![], vec![]);
    for triple in std::iter::once(crate::dev::host_triple()).chain(targets.iter().copied()) {
        eprintln!("ax release: building aio-rs {version} for {triple}");
        let mut cargo = std::process::Command::new("cargo");
        cargo.args(["build", "--release", "--locked", "-p", "aio-rs-cli", "--target", triple, "--message-format=json"]);
        crate::build::remap(&mut cargo, root);
        // Its own checkout's path stays out of it (`create::this_checkout`).
        cargo.env("AIORT_RELEASED", "1");
        let o = cargo
            .current_dir(root)
            .stderr(std::process::Stdio::inherit())
            .output()
            .map_err(|e| format!("cargo: {e}"))?;
        if !o.status.success() {
            return Err(format!("ax for {triple} did not build (cargo said why, above)"));
        }
        let exe = String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(|m| m["target"]["name"] == "ax" && m["executable"].is_string())
            .and_then(|m| m["executable"].as_str().map(PathBuf::from))
            .ok_or(format!("cargo built no ax for {triple}"))?;
        crate::build::no_home_paths(&std::fs::read(&exe).map_err(|e| e.to_string())?, &exe)?;
        let scratch = out.join(format!(".sign-{triple}"));
        aio::paths::private_dir(&scratch).map_err(|e| e.to_string())?;
        let signed = if triple.contains("-windows-") {
            sign_windows(&exe, &scratch).map(|(p, line)| (Some(p), line))
        } else if triple.contains("-apple-") {
            sign_macos(&exe, &scratch).map(|line| (None, line))
        } else {
            Ok((None, String::new()))
        };
        let name = aio_archive(version, triple);
        let packed = signed.and_then(|(p, line)| pack(p.as_deref().unwrap_or(&exe), &out.join(&name)).map(|_| line));
        _ = std::fs::remove_dir_all(&scratch);
        signing.push(packed?);
        let bytes = std::fs::read(out.join(&name)).map_err(|e| e.to_string())?;
        made.push((triple.to_string(), name, sha256_hex(&bytes)));
    }
    let names: Vec<String> = std::fs::read_dir(&out)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    let built: Vec<String> = made.iter().map(|(t, ..)| t.clone()).collect();
    let elsewhere = built_elsewhere(&names, version, &built);
    for (t, f) in &elsewhere {
        let bytes = std::fs::read(out.join(f)).map_err(|e| format!("{f}: {e}"))?;
        made.push((t.clone(), f.clone(), sha256_hex(&bytes)));
    }
    let pins: Vec<(String, String)> = made.iter().map(|(t, _, s)| (t.clone(), s.clone())).collect();
    let url = if url.is_empty() { aio_release_url(version) } else { url.to_string() };
    let (sh, ps1) = installers(version, &url, &pins);
    for (f, text) in [("install.sh", sh), ("install.ps1", ps1)] {
        std::fs::write(out.join(f), text).map_err(|e| format!("{f}: {e}"))?;
    }
    let files = every_file(&out)?;
    std::fs::write(out.join("SHA256SUMS"), sums(&files)).map_err(|e| format!("SHA256SUMS: {e}"))?;
    // Read back: what is there is what was pinned.
    for (_, f, s) in &made {
        if !files.iter().any(|(g, got)| g == f && got == s) {
            return Err(format!("{f} changed after it was made (pinned {s})"));
        }
    }
    signing.retain(|l| !l.is_empty());
    // Both Mac builds say the same about codesign: once.
    signing.dedup();
    signing.push(sign_sums(&out, update_key.as_deref())?);
    let list: Vec<String> = made
        .iter()
        .map(|(t, f, s)| {
            let from = if built.contains(t) { "" } else { " (built elsewhere)" };
            format!("  {f}  sha256 {s}{from}")
        })
        .collect();
    Ok(format!(
        "aio-rs {version} in {}:\n{}\n  SHA256SUMS ({} files), install.sh, install.ps1 (each build's SHA-256 pinned)\n\
         signing:\n{}\n\
         try it: sh {}/install.sh (or upload the folder to {} and curl -fsSL <url>/install.sh | sh)",
        out.display(),
        list.join("\n"),
        files.len(),
        signing.join("\n"),
        out.display(),
        url
    ))
}

/// `ax update <app>`: what the installed app's updates are doing.
pub fn status(id: &str) -> Result<String, String> {
    let d = aio_shell::update::dir(id);
    if !d.exists() {
        return Err(format!("{id} is not installed from a bundle (no {})", d.display()));
    }
    let read = |f: &str| std::fs::read_to_string(d.join(f)).ok().map(|t| t.trim().to_string());
    // Linux only: macOS and Windows have no `current` link (the version is in installed.json).
    let current = std::fs::read_link(d.join("current")).ok();
    let mut versions: Vec<String> = std::fs::read_dir(d.join("versions"))
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str().map(String::from)).collect())
        .unwrap_or_default();
    versions.retain(|v| !v.starts_with('.'));
    versions.sort_by(|a, b| up::compare(b, a).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<String> = current.map(|c| format!("{id}: current → {}", c.display())).into_iter().collect();
    out.push(format!("versions: {}", versions.join(", ")));
    for (f, what) in [
        ("installed.json", "installed"),
        ("pending.json", "installing"),
        ("first-boot", "waiting for the first start of"),
        ("update-failed", "a failed update to undo"),
        ("dismissed.json", "never offered again"),
    ] {
        if let Some(t) = read(f) {
            out.push(format!("{what}: {t}"));
        }
    }
    Ok(out.join("\n"))
}

/// `ax update <app> --from-git <ref>`, in the app's workspace: `<ref>`
/// of its `[updates] url` when that is a git source, else of the git
/// checkout it is in, built and staged unsigned in the installed app
/// (`aio_shell::update::from_git`); `apps`: the apps' data homes.
pub fn from_git(name: &str, r: &str, apps: &Path) -> Result<String, String> {
    let pkg = crate::dev::metadata()
        .and_then(|m| crate::dev::locate(&m, name))
        .map_err(|e| format!("{e}\n(--from-git builds the app of this workspace)"))?;
    let (app, cfg) = crate::build::app_of(&pkg)?;
    let d = aio_shell::update::dir(&app.id);
    let installed: aio::serde_json::Value =
        std::fs::read(d.join("installed.json")).ok().and_then(|b| aio::serde_json::from_slice(&b).ok()).ok_or_else(
            || format!("{} is not installed from a bundle (no {})", app.id, d.join("installed.json").display()),
        )?;
    let version = installed["version"].as_str().ok_or("installed.json names no version")?;
    let remote = match cfg.updates.url.as_deref().and_then(up::git_remote) {
        Some(r) => r.to_string(),
        None => {
            let here = crate::build::app_dir(&pkg)?;
            let out = std::process::Command::new("git")
                .args(["rev-parse", "--show-toplevel"])
                .current_dir(&here)
                .output()
                .map_err(|e| format!("git: {e}"))?;
            if !out.status.success() {
                return Err(format!("{} is in no git checkout and [updates] url is no git source", here.display()));
            }
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
    };
    let data = apps.join(&app.id).join("data");
    aio_shell::update::from_git(&cfg, (&remote, r), &d, &data, version, &|src| {
        aio_shell::update::aio_build(&app.id, src)
    })
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_made_once_and_rotated_on_purpose() {
        let d = tempfile::tempdir().unwrap();
        let said = keygen(d.path(), "notes", false).unwrap();
        assert!(said.contains("keys = [\"ed25519:"));
        assert!(
            said.contains("[capabilities]\nnetwork = [") && said.contains("XDG_CONFIG_HOME"),
            "all a release needs"
        );
        let key = d.path().join("notes.key");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&key).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(keygen(d.path(), "notes", false).unwrap_err().contains("exists"));
        let first = up::public_key(&read_key(&key).unwrap()).unwrap();
        let rotated = keygen(d.path(), "notes", true).unwrap();
        assert!(rotated.contains(&first), "both keys go in aio.toml: {rotated}");
        assert_eq!(up::public_key(&read_key(&d.path().join("notes.previous.key")).unwrap()).unwrap(), first);
    }

    #[test]
    fn key_signs_with_that_file_and_its_previous_key() {
        let d = tempfile::tempdir().unwrap();
        keygen(d.path(), "ci", false).unwrap();
        keygen(d.path(), "ci", true).unwrap();
        let file = d.path().join("ci.key");
        let ask = Ask { key: Some(&file), ..Default::default() };
        let public = up::public_key(&read_key(&file).unwrap()).unwrap();
        let u = aio::config::Updates { keys: vec![public], ..Default::default() };
        let (key, signers) = signing_keys("notes", "notes", &u, &ask).unwrap();
        assert_eq!((key, signers.len()), (read_key(&file).unwrap(), 2), "the rotated key signs the roster too");
        let other = aio::config::Updates { keys: vec![], ..Default::default() };
        assert!(signing_keys("notes", "notes", &other, &ask).unwrap_err().contains("don't include this signing key"));
        let none = d.path().join("none.key");
        let e = signing_keys("notes", "notes", &u, &Ask { key: Some(&none), ..Default::default() }).unwrap_err();
        assert!(e.contains("none.key") && !e.contains("ax keygen"), "{e}");
    }

    #[test]
    fn an_unsigned_release_needs_no_key_and_is_trusted_by_no_app() {
        let u = aio::config::Updates { keys: vec![], ..Default::default() };
        let unsigned = Ask { unsigned: true, ..Default::default() };
        let (key, signers) = signing_keys("no-key-app", "no-key-app", &u, &unsigned).unwrap();
        assert_eq!(signers, std::slice::from_ref(&key));
        let (other, _) = signing_keys("no-key-app", "no-key-app", &u, &unsigned).unwrap();
        assert_ne!(key, other, "made for each release, kept nowhere");
        assert!(signed(true).contains("never to publish") && signed(false) == "signed");
    }

    /// A Linux bundle of notes `v`: `notes-<v>/` packed, its binary starting with `change`.
    #[cfg(target_os = "linux")]
    fn bundle(dir: &Path, v: &str, change: &[u8]) -> Vec<u8> {
        let top = dir.join(format!("src-{v}/notes-{v}"));
        std::fs::create_dir_all(&top).unwrap();
        let mut bin: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        bin[..change.len()].copy_from_slice(change);
        std::fs::write(top.join("notes"), bin).unwrap();
        std::fs::write(top.join("install.sh"), "#!/bin/sh\n").unwrap();
        let out = dir.join(format!("notes-{v}-linux-x64.tar.gz"));
        let st = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&out)
            .arg("-C")
            .arg(top.parent().unwrap())
            .arg(format!("notes-{v}"))
            .status();
        assert!(st.unwrap().success());
        std::fs::read(out).unwrap()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_release_carries_signed_patches_from_the_versions_asked() {
        let d = tempfile::tempdir().unwrap();
        let out = d.path();
        let (k, _) = up::keygen().unwrap();
        bundle(out, "1.0.0", b"one");
        bundle(out, "0.9.0", b"nine");
        let new = bundle(out, "1.1.0", b"two");
        let release = up::Release {
            format: up::FORMAT,
            id: "notes".into(),
            version: "1.1.0".into(),
            channel: "stable".into(),
            os: "linux".into(),
            arch: std::env::consts::ARCH.into(),
            file: "notes-1.1.0-linux-x64.tar.gz".into(),
            sha256: sha256_hex(&new),
            size: new.len() as u64,
            released_at: "2026-10-28T00:00:00.000Z".into(),
            notes: String::new(),
            data: up::Contract::default(),
            roster: None,
            min_from: None,
            commit: None,
        };
        let m = up::sign(release, &k).unwrap();
        let name = |v: &str| format!("notes-{v}-linux-x64.tar.gz");
        let unpack = |b: &[u8], d: &Path| aio_shell::update::unpack_for("linux", b, d);
        let named = format!("linux-{}", std::env::consts::ARCH);
        let files = patches(out, &m, &new, &["1.0.0", "0.9.0"], (&named, &name, &unpack), &k).unwrap();
        let json = out.join(format!("linux-{}.patches.json", std::env::consts::ARCH));
        assert_eq!(files[0], json);
        assert_eq!(files.len(), 3, "the list and one patch per version");
        let list: up::Patches = aio::serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
        let p = up::patch_for(&list, &m, "1.0.0").unwrap();
        assert!(p.size < 2_000, "{} bytes", p.size);
        // What an app running 1.0.0 makes of it: 1.1.0, as unpacked from its bundle.
        let (here, made, want) = (out.join("here"), out.join("made"), out.join("want"));
        for (dir, b) in [(&here, std::fs::read(out.join(name("1.0.0"))).unwrap()), (&want, new.clone())] {
            std::fs::create_dir_all(dir).unwrap();
            aio_shell::update::unpack_for("linux", &b, dir).unwrap();
        }
        aio_shell::delta::apply(&std::fs::read(out.join(&p.file)).unwrap(), &here, &made).unwrap();
        assert_eq!(std::fs::read(made.join("notes")).unwrap(), std::fs::read(want.join("notes")).unwrap());
        assert!(!out.join(".patch-1.0.0").exists(), "the scratch is gone");
        // A version whose bundle isn't here is said; none asked: no list left to upload.
        assert!(
            patches(out, &m, &new, &["0.1.0"], (&named, &name, &unpack), &k)
                .unwrap_err()
                .contains("--patch-from 0.1.0")
        );
        assert_eq!(patches(out, &m, &new, &[], (&named, &name, &unpack), &k).unwrap(), Vec::<PathBuf>::new());
        assert!(!json.exists());
    }

    #[test]
    fn a_release_old_installs_cant_read_is_warned_of() {
        let r = |commit: Option<&str>, min: Option<&str>| up::Release {
            commit: commit.map(String::from),
            min_from: min.map(String::from),
            format: up::FORMAT,
            id: "notes".into(),
            version: "1.1.0".into(),
            channel: "stable".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            file: "notes-1.1.0-linux-x64.tar.gz".into(),
            sha256: String::new(),
            size: 0,
            released_at: String::new(),
            notes: String::new(),
            data: up::Contract::default(),
            roster: None,
        };
        assert_eq!(unread_before_0_3(&r(None, None)), None, "a plain release: no warning");
        let w = unread_before_0_3(&r(Some("9f3ac2b1"), Some("1.0.0"))).unwrap();
        assert!(
            w.starts_with("warning: this release names its commit and a minFrom: an install on aio-rs older than 0.3")
        );
        assert!(!w.contains('\n') && w.contains("publish a plain release"), "{w}");
        assert!(unread_before_0_3(&r(None, Some("1.0.0"))).unwrap().contains("names a minFrom:"));
    }

    #[test]
    fn min_from_is_a_version_at_most_the_releases_own() {
        assert_eq!(min_from(None, "1.2.0"), Ok(None));
        assert_eq!(min_from(Some("1.1.40"), "1.2.0"), Ok(Some("1.1.40".into())));
        assert!(min_from(Some("soon"), "1.2.0").unwrap_err().contains("not a version"));
        assert!(min_from(Some("1.3.0"), "1.2.0").unwrap_err().contains("above this release's version"));
    }

    /// aio am publish --dir, --no-build (dist/ as it is), --data.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn a_release_is_laid_out_where_asked_reading_the_data_it_was_given() {
        let d = tempfile::tempdir().unwrap();
        let dist = d.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        let (k, public) = up::keygen().unwrap();
        let mut cfg = aio::config::Config::default();
        cfg.updates.keys = vec![public];
        let linux = crate::platform::of("linux", "x86_64").unwrap();
        let app = crate::build::App { id: "notes".into(), name: "Notes".into(), version: "1.1.0".into(), icon: None };
        let b = crate::build::Built {
            app,
            cfg,
            binary: dist.join("notes"),
            dist: dist.clone(),
            platform: linux,
            runtime: None,
            look: Default::default(),
        };
        let new = bundle(&dist, "1.1.0", b"two");
        // --dir srv: the channel's dir there, an earlier release in it.
        let out = d.path().join("srv/beta");
        std::fs::create_dir_all(&out).unwrap();
        bundle(&out, "1.0.0", b"one");
        let file = d.path().join("contract.json");
        std::fs::write(&file, r#"{ "schema": 1, "cells": { "notes": 2 } }"#).unwrap();
        let data = contract_in(&file).unwrap();
        assert_eq!(data.cells["notes"], 2);
        let built = dist.join("notes-1.1.0-linux-x64.tar.gz");
        std::fs::write(crate::manifest::path(&dist, "notes"), r#"{ "commit": "9f3ac2b1" }"#).unwrap();
        let (files, m) = lay_out(
            &b,
            (&built, "linux-x86_64"),
            &out,
            ("beta", "fixes", data.clone(), Some("1.0.0"), true),
            (&k, std::slice::from_ref(&k)),
            &["1.0.0"],
        )
        .unwrap();
        let json = out.join("linux-x86_64.json");
        assert_eq!(
            files[..2],
            [json.display().to_string(), out.join("notes-1.1.0-linux-x64.tar.gz").display().to_string()]
        );
        assert_eq!(files.len(), 4, "and the patch list and its patch: {files:?}");
        assert_eq!(std::fs::read(out.join("notes-1.1.0-linux-x64.tar.gz")).unwrap(), new);
        assert!(built.exists(), "dist/ keeps what it held");
        let read: up::Manifest = aio::serde_json::from_slice(&std::fs::read(&json).unwrap()).unwrap();
        assert_eq!(read, m);
        let r = &m.release;
        assert_eq!((r.channel.as_str(), r.notes.as_str(), &r.data), ("beta", "fixes", &data));
        assert_eq!((r.min_from.as_deref(), r.commit.as_deref()), (Some("1.0.0"), Some("9f3ac2b1")), "both signed in");
        assert_eq!((r.sha256.clone(), r.size), (sha256_hex(&new), new.len() as u64));
        let expect = up::Expect { id: "notes", channel: "beta", os: "linux", arch: "x86_64" };
        assert!(up::check(&m, &b.cfg.updates.keys, &expect).is_ok(), "an app on beta takes it");
        // The same version with other bytes is refused; the same bytes again go over it.
        bundle(&dist, "1.1.0", b"rebuilt");
        let again = || {
            let ask = ("beta", "fixes", data.clone(), None, false);
            lay_out(&b, (&built, "linux-x86_64"), &out, ask, (&k, std::slice::from_ref(&k)), &[])
        };
        let e = again().unwrap_err();
        assert!(e.starts_with("1.1.0 is released here already with other bytes") && e.contains("--allow-empty"), "{e}");
        assert_eq!(std::fs::read(out.join("notes-1.1.0-linux-x64.tar.gz")).unwrap(), new, "nothing replaced");
        std::fs::write(&built, &new).unwrap();
        assert_eq!(again().unwrap().1.release.sha256, sha256_hex(&new));
        // A contract file that isn't one is refused, not published as nothing.
        assert!(contract_in(&d.path().join("none.json")).unwrap_err().starts_with("--data"));
        std::fs::write(&file, r#"{ "cells": {} }"#).unwrap();
        assert!(contract_in(&file).unwrap_err().contains("not a data contract"));
    }

    /// aio's appimage update target: the AppImage, signed under its own name.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_appimage_is_released_beside_its_own_manifest_with_its_patches() {
        let d = tempfile::tempdir().unwrap();
        let (k, public) = up::keygen().unwrap();
        let mut cfg = aio::config::Config::default();
        cfg.updates.keys = vec![public];
        let p = |n: &str| crate::platform::ALL.iter().find(|p| p.name == n).unwrap();
        let app = crate::build::App { id: "notes".into(), name: "Notes".into(), version: "1.1.0".into(), icon: None };
        let mut b = crate::build::Built {
            app,
            cfg,
            binary: d.path().join("notes"),
            dist: d.path().to_path_buf(),
            platform: p("linux"),
            runtime: None,
            look: Default::default(),
        };
        let ask = |no_build, patch_from| Ask { no_build, patch_from, appimage: true, ..Ask::default() };
        let tgz = d.path().join("notes-1.1.0-linux-x64.tar.gz");
        assert!(appimage_of(&b, &tgz, &ask(true, vec![])).unwrap_err().contains("ax build notes --appimage"));
        // Two images that differ by a few bytes, as two builds a commit apart.
        let pad: Vec<u8> = (0..60_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
        let (old, mut new) = (pad.clone(), pad);
        new[30_000..30_004].copy_from_slice(b"1.1.");
        let srv = d.path().join("srv/stable");
        std::fs::create_dir_all(&srv).unwrap();
        std::fs::write(srv.join("notes-1.0.0-x86_64.AppImage"), &old).unwrap();
        let image = d.path().join("notes-1.1.0-x86_64.AppImage");
        std::fs::write(&image, &new).unwrap();
        assert_eq!(appimage_of(&b, &tgz, &ask(true, vec![])).unwrap(), image);
        let (files, m) = lay_out(
            &b,
            (&image, "linux-x86_64.appimage"),
            &d.path().join("srv/stable"),
            ("stable", "", up::Contract::default(), None, false),
            (&k, std::slice::from_ref(&k)),
            &["1.0.0"],
        )
        .unwrap();
        let patch = srv.join("notes-1.1.0-from-1.0.0.patch");
        let want = [
            srv.join("linux-x86_64.appimage.json"),
            srv.join("notes-1.1.0-x86_64.AppImage"),
            srv.join("linux-x86_64.appimage.patches.json"),
            patch.clone(),
        ];
        assert_eq!(files, want.map(|p| p.display().to_string()));
        // Its patch, signed under the name an AppImage install asks for, makes the new image from the old.
        let list: up::Patches =
            aio::serde_json::from_slice(&std::fs::read(srv.join("linux-x86_64.appimage.patches.json")).unwrap())
                .unwrap();
        let one = up::patch_for(&list, &m, "1.0.0").unwrap();
        assert!(one.size < 2_000, "{} bytes", one.size);
        let (here, made) = (d.path().join("here"), d.path().join("made"));
        aio_shell::update::unpack_appimage("notes", &old, &here).unwrap();
        aio_shell::delta::apply(&std::fs::read(&patch).unwrap(), &here, &made).unwrap();
        assert_eq!(std::fs::read(made.join("notes.AppImage")).unwrap(), new);
        assert_eq!(m.release.file, "notes-1.1.0-x86_64.AppImage");
        // A plain release names no commit or minFrom: an install on aio-rs 0.2 reads it.
        assert_eq!((m.release.commit.as_ref(), m.release.min_from.as_ref()), (None, None));
        let served = std::fs::read_to_string(srv.join("linux-x86_64.appimage.json")).unwrap();
        assert!(!served.contains("commit") && !served.contains("minFrom"), "{served}");
        assert!(signed_commit(true, d.path(), "notes").unwrap_err().contains("names no commit"));
        let expect = up::Expect { id: "notes", channel: "stable", os: "linux", arch: "x86_64" };
        assert!(up::check(&m, &b.cfg.updates.keys, &expect).is_ok());
        b.platform = p("windows");
        assert!(appimage_of(&b, &tgz, &ask(true, vec![])).unwrap_err().contains("Linux package"));
    }

    #[test]
    fn the_workflow_releases_every_os_from_a_tag_and_publishes_files_before_manifests() {
        let w = workflow("notes", "notes", "stable", Some(("https://example.org/me/aio", "abc123")), (false, false));
        for want in [
            "tags: [\"v*\"]",
            "cargo install --locked --git \"https://example.org/me/aio\" --rev \"abc123\" aio-rs-cli",
            "ax release notes --channel stable $FLAGS",
            "flags: \"--windows\"",
            "os: macos-15",
            "${{ secrets.AIO_SIGNING_KEY }}",
            "keys/notes.key",
            "gh release create \"$GITHUB_REF_NAME\" --draft --verify-tag",
            "--latest=\"$latest\"",
            "\"version\\\": \\\"${GITHUB_REF_NAME#v}",
        ] {
            assert!(w.contains(want), "{want}\n{w}");
        }
        // Files, then manifests.
        assert!(w.find("! -name '*.json'").unwrap() < w.find("out/*.json --clobber").unwrap());
        // Another channel: its own moving prerelease, never "latest".
        let beta = workflow("notes", "notes", "beta", None, (false, false));
        assert!(beta.contains("gh release create beta --draft --prerelease") && !beta.contains("--latest"));
        for w in [&w, &beta] {
            assert!(!w.contains("{{ os") && !w.contains("${{{{"), "no format braces left");
            assert!(w.contains("path: app\n") && w.contains("path: app/dist/"), "the app checked out in app/");
        }
        // aio's `ship github`: the channel on GitHub Pages too, the web build at its root.
        assert!(!w.contains("deploy-pages") && !w.contains("\n  web:"), "neither unasked");
        let pages = workflow("notes", "notes", "beta", None, (true, false));
        let site = workflow("notes", "notes", "beta", None, (true, true));
        for want in [
            "mkdir -p site/beta && cp out/* site/beta/",
            "uses: actions/upload-pages-artifact@v3\n        with:\n          path: site",
            "  pages:\n    needs: publish",
            "pages: write\n      id-token: write",
            "url: ${{ steps.deploy.outputs.page_url }}",
            "uses: actions/deploy-pages@v4",
            "pattern: notes-[0-9]*",
        ] {
            assert!(pages.contains(want) && site.contains(want), "{want}\n{pages}");
        }
        assert!(!pages.contains("ax build notes --web"));
        for want in [
            "  web:\n    runs-on: ubuntu-24.04",
            "rustup target add wasm32-unknown-unknown",
            "run: ax build notes --web",
            "path: app/dist/notes-web",
            "needs: [build, web]",
            "name: web\n          path: site",
        ] {
            assert!(site.contains(want), "{want}\n{site}");
        }
        // The web build lands first, then the channel's files beside it.
        assert!(site.find("path: site\n").unwrap() < site.find("mkdir -p site/beta").unwrap());
        for w in [&pages, &site] {
            assert!(!w.contains("{{ os") && !w.contains("${{{{") && !w.contains("{get_aio}"), "no format braces left");
        }
        // A path dependency (`ax create --aio`): aio-rs at this ax's tag beside the app.
        let v = env!("CARGO_PKG_VERSION");
        for want in [
            "repository: ${{ vars.AIO_REPO || 'riagentic/aio-rs' }}".to_string(),
            format!("ref: ${{{{ vars.AIO_REF || 'v{v}' }}}}"),
            "path: aio-rs\n".into(),
            format!("-exec perl -pi -e '{REPOINT}' {{}} +"),
            "working-directory: aio-rs\n        run: cargo install --locked --path crates/aio-cli".into(),
        ] {
            assert!(beta.contains(&want), "{want}\n{beta}");
        }
        assert!(!w.contains("path: aio-rs") && !w.contains("AIO_REF"), "a git dependency needs no checkout");
    }

    /// The workflow's edit, run on what `ax create --aio` writes: every aio-rs
    /// path goes to the checkout, the app's own path stays.
    #[test]
    fn the_workflow_points_create_s_aio_paths_at_the_checkout() {
        if std::process::Command::new("perl").arg("-v").output().is_err() {
            eprintln!("no perl here: skipped");
            return;
        }
        for t in ["app", "cli", "service"] {
            for (rel, text) in {
                let aio = crate::create::Aio::Path("../my aio/crates".into());
                crate::create::files(&crate::create::New { template: t, aio, ..crate::create::New::of("notes") })
                    .unwrap()
            } {
                if !rel.ends_with("Cargo.toml") {
                    continue;
                }
                let mut p = std::process::Command::new("perl")
                    .args(["-pe", REPOINT])
                    .env("GITHUB_WORKSPACE", "/w")
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .spawn()
                    .unwrap();
                std::io::Write::write_all(&mut p.stdin.take().unwrap(), text.as_bytes()).unwrap();
                let out = String::from_utf8(p.wait_with_output().unwrap().stdout).unwrap();
                assert!(!out.contains("my aio"), "{rel}: {out}");
                assert!(out.contains(r#"package = "aio-rs", path = "/w/aio-rs/crates/aio""#), "{rel}: {out}");
                if text.contains("aio-rs-shell") {
                    assert!(out.contains(r#"package = "aio-rs-shell", path = "/w/aio-rs/crates/aio-shell""#), "{out}");
                }
                assert_eq!(out.contains(r#"path = "..""#), text.contains(r#"path = "..""#));
            }
        }
    }

    #[test]
    fn aio_comes_from_where_the_apps_aio_rt_comes_from() {
        let pkg = |src: aio::serde_json::Value| aio::serde_json::json!({ "dependencies": [{ "name": "serde" }, { "name": "aio-rs", "source": src }] });
        let from = pkg("git+https://example.org/me/aio?rev=abc#abc1234def".into());
        assert_eq!(aio_source(&from), Some(("https://example.org/me/aio".into(), "abc1234def".into())));
        assert_eq!(aio_source(&pkg(aio::serde_json::Value::Null)), None, "a path dependency");
        assert_eq!(aio_source(&pkg("registry+https://example.org/index".into())), None);
    }

    #[test]
    fn each_apk_is_signed_for_its_cpu_and_the_play_bundle_is_left_out() {
        let d = tempfile::tempdir().unwrap();
        let (k, public) = up::keygen().unwrap();
        let (built, out) = (d.path().join("dist"), d.path().join("dist/stable"));
        std::fs::create_dir_all(&built).unwrap();
        let names = [
            "notes-1.1.0-android-arm64-v8a.apk",
            "notes-1.1.0-android-armeabi-v7a.apk",
            "notes-1.1.0-android-x86_64.apk",
            "notes-1.1.0-android.aab",
            "notes-1.1.0-android-universal.apk",
        ];
        let apks: Vec<PathBuf> = names.iter().map(|n| built.join(n)).collect();
        for (p, body) in apks.iter().zip(["arm", "a32", "x86", "play", "all"]) {
            std::fs::write(p, body).unwrap();
        }
        let base = up::Release {
            format: up::FORMAT,
            id: "notes".into(),
            version: "1.1.0".into(),
            channel: "stable".into(),
            os: String::new(),
            arch: String::new(),
            file: String::new(),
            sha256: String::new(),
            size: 0,
            released_at: "2026-10-29T00:00:00.000Z".into(),
            notes: String::new(),
            data: up::Contract::default(),
            roster: None,
            min_from: None,
            commit: None,
        };
        let files = android_releases(&out, &apks, &base, &k, false).unwrap();
        let want: Vec<PathBuf> =
            ["android-aarch64.json", names[0], "android-arm.json", names[1], "android-x86_64.json", names[2]]
                .iter()
                .map(|f| out.join(f))
                .collect();
        assert_eq!(files, want);
        assert!(apks[3].exists() && !out.join(names[3]).exists(), "the .aab stays for Play");
        assert!(apks[4].exists() && !out.join(names[4]).exists(), "the universal APK is no update");
        for (arch, file, body) in [("aarch64", names[0], "arm"), ("arm", names[1], "a32"), ("x86_64", names[2], "x86")]
        {
            let m: up::Manifest =
                aio::serde_json::from_slice(&std::fs::read(out.join(format!("android-{arch}.json"))).unwrap()).unwrap();
            let expect = up::Expect { id: "notes", channel: "stable", os: "android", arch };
            up::check(&m, std::slice::from_ref(&public), &expect).unwrap();
            assert_eq!((m.release.file.as_str(), m.release.size), (file, 3));
            assert_eq!(m.release.sha256, sha256_hex(body.as_bytes()));
            let wrong = up::Expect { arch: "x86", ..expect };
            assert!(up::check(&m, std::slice::from_ref(&public), &wrong).is_err(), "never for another CPU");
        }
        let odd = [built.join("notes-1.1.0-android-mips.apk")];
        std::fs::write(&odd[0], "").unwrap();
        assert!(android_releases(&out, &odd, &base, &k, false).unwrap_err().contains("not an APK of a known CPU type"));
        // --no-build --dir srv: the APKs ax build left in dist/, copied, dist/ as it was.
        assert!(apks_built(&built, "notes", "1.1.0", "notes").unwrap_err().contains("--targets android"));
        let left: Vec<PathBuf> = names[..3].iter().map(|n| built.join(n)).collect();
        for p in &left {
            std::fs::write(p, "apk").unwrap();
        }
        assert_eq!(apks_built(&built, "notes", "1.1.0", "notes").unwrap(), left);
        let srv = d.path().join("srv/beta");
        let files = android_releases(&srv, &left, &base, &k, true).unwrap();
        assert_eq!(files.len(), 6);
        assert!(files.iter().all(|f| f.starts_with(&srv) && f.is_file()), "{files:?}");
        assert!(left.iter().all(|p| p.is_file()), "dist/ keeps what ax build made");
    }

    #[test]
    fn aios_installers_pin_each_builds_sha_for_their_os() {
        assert_eq!(aio_archive("0.1.0", "x86_64-unknown-linux-gnu"), "aio-rs-0.1.0-x86_64-unknown-linux-gnu.tar.gz");
        assert_eq!(aio_archive("0.1.0", "x86_64-pc-windows-gnu"), "aio-rs-0.1.0-x86_64-pc-windows-gnu.zip");
        let pins = [("x86_64-unknown-linux-gnu".into(), "a1".into()), ("x86_64-pc-windows-gnu".into(), "b2".into())];
        let (sh, ps1) = installers("0.1.0", "https://example.org/aio", &pins);
        assert!(sh.contains("version=0.1.0\n") && sh.contains("  x86_64-unknown-linux-gnu) want=a1 ;;"), "{sh}");
        assert!(sh.contains("${AIO_DOWNLOAD:-https://example.org/aio}") && !sh.contains("b2"));
        assert!(
            ps1.contains("$version = '0.1.0'")
                && ps1.contains("  'x86_64-pc-windows-gnu' = 'b2'")
                && !ps1.contains("a1")
        );
        assert!(!sh.contains("@VERSION@") && !sh.contains("@URL@") && !sh.contains("@PINS@"));
        assert!(!ps1.contains("@VERSION@") && !ps1.contains("@URL@") && !ps1.contains("@PINS@"));
        assert_eq!(sums(&[("f.zip".into(), "ab".into())]), "ab  f.zip\n");
        assert_eq!(aio_release_url("0.1.0"), "https://github.com/riagentic/aio-rs/releases/download/v0.1.0");
    }

    #[test]
    fn an_aio_archive_a_mac_made_is_pinned_beside_the_ones_built_here() {
        let names: Vec<String> = [
            "aio-rs-0.1.0-aarch64-apple-darwin.tar.gz",
            "aio-rs-0.1.0-x86_64-unknown-linux-gnu.tar.gz",
            "aio-rs-0.0.9-x86_64-apple-darwin.tar.gz",
            "aio-rs-0.1.0-aarch64-apple-darwin.zip",
            "aio-0.1.0-x86_64-apple-darwin.tar.gz",
            "install.sh",
            "template",
        ]
        .map(String::from)
        .into();
        let found = built_elsewhere(&names, "0.1.0", &["x86_64-unknown-linux-gnu".into()]);
        assert_eq!(found, [("aarch64-apple-darwin".into(), "aio-rs-0.1.0-aarch64-apple-darwin.tar.gz".into())]);
    }

    #[test]
    fn sha256sums_lists_every_file_and_is_signed_only_with_a_key() {
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("rel");
        std::fs::create_dir_all(out.join("template/android")).unwrap();
        std::fs::write(out.join("aio.tar.gz"), "a").unwrap();
        std::fs::write(out.join("template/android/t.apk"), "b").unwrap();
        std::fs::write(out.join(".sign-x"), "scratch").unwrap();
        std::fs::write(out.join("SHA256SUMS.sig"), "old").unwrap();
        let files = every_file(&out).unwrap();
        let listed: Vec<&str> = files.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(listed, ["aio.tar.gz", "template/android/t.apk"]);
        std::fs::write(out.join("SHA256SUMS"), sums(&files)).unwrap();

        let said = sign_sums(&out, None).unwrap();
        assert!(said.contains("unsigned: no keys configured (AIO_RELEASE_KEY"), "{said}");
        assert!(!out.join("SHA256SUMS.sig").exists(), "no key: an earlier signature goes");

        let keys = d.path().join("keys");
        keygen_aio(&keys, false).unwrap();
        let key = keys.join("aio.release.key");
        let said = sign_sums(&out, Some(&key)).unwrap();
        let public = up::public_key(&read_key(&key).unwrap()).unwrap();
        assert!(said.ends_with(&format!("signed by {public}")), "{said}");
        let sig = std::fs::read_to_string(out.join("SHA256SUMS.sig")).unwrap();
        let body = std::fs::read(out.join("SHA256SUMS")).unwrap();
        assert!(up::verify(&public, &body, sig.trim()));
        assert!(!up::verify(&public, b"other sums", sig.trim()));
        assert!(keygen_aio(&keys, false).unwrap_err().contains("exists"), "never over a kept key");
    }

    #[cfg(unix)]
    #[test]
    fn install_sh_installs_ax_only_when_its_sha_matches_then_runs_setup() {
        let d = tempfile::tempdir().unwrap();
        let (rel, home) = (d.path().join("rel"), d.path().join("home"));
        std::fs::create_dir_all(home.join(".cargo/bin")).unwrap();
        std::fs::create_dir_all(&rel).unwrap();
        let fake = d.path().join("ax");
        std::fs::write(&fake, "#!/bin/sh\necho \"ran $*\" > \"$(dirname \"$0\")/said\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        let triple = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => "aarch64-apple-darwin",
            ("macos", _) => "x86_64-apple-darwin",
            (_, "aarch64") => "aarch64-unknown-linux-gnu",
            _ => "x86_64-unknown-linux-gnu",
        };
        let name = aio_archive("9.9.9", triple);
        pack(&fake, &rel.join(&name)).unwrap();
        let listed = std::process::Command::new("tar").arg("-tvzf").arg(rel.join(&name)).output().unwrap();
        let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
        assert!(listed.starts_with("-rwxr-xr-x"), "ax runnable by all, not as its umask left it: {listed}");
        let sha = sha256_hex(&std::fs::read(rel.join(&name)).unwrap());
        let install = |pin: &str| {
            let (sh, _) = installers("9.9.9", "", &[(triple.into(), pin.into())]);
            std::fs::write(rel.join("install.sh"), sh).unwrap();
            let o = std::process::Command::new("sh")
                .arg(rel.join("install.sh"))
                .env("HOME", &home)
                .env_remove("CARGO_HOME")
                .env_remove("AIO_DOWNLOAD")
                .output()
                .unwrap();
            (o.status.success(), String::from_utf8_lossy(&o.stderr).into_owned())
        };
        let bin = home.join(".cargo/bin");
        let (ok, said) = install(&"0".repeat(64));
        assert!(!ok && said.contains("expected 0000") && said.contains("not installed"), "{said}");
        assert!(!bin.join("ax").exists(), "nothing installed, nothing ran");
        let (ok, said) = install(&sha);
        assert!(ok, "{said}");
        assert_eq!(std::fs::read_to_string(bin.join("said")).unwrap(), "ran setup\n", "ax setup ran after it");
        // curl | sh: the script on stdin, the release's files in a folder
        // (air-gapped: AIO_DOWNLOAD names it), run from elsewhere.
        let (sh, _) = installers("9.9.9", "https://192.0.2.1/never", &[(triple.into(), sha.clone())]);
        let piped = |script: &[u8], from: &Path| {
            use std::io::Write;
            std::fs::remove_file(bin.join("ax")).ok();
            let mut c = std::process::Command::new("sh")
                .current_dir(d.path())
                .env("HOME", &home)
                .env_remove("CARGO_HOME")
                .env("AIO_DOWNLOAD", from)
                .stdin(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            c.stdin.take().unwrap().write_all(script).unwrap();
            let o = c.wait_with_output().unwrap();
            (o.status.success(), bin.join("ax").exists(), String::from_utf8_lossy(&o.stderr).into_owned())
        };
        let (ok, installed, said) = piped(sh.as_bytes(), &rel);
        assert!(ok && installed, "from a folder, piped: {said}");
        let cut = sh.find("tar -xzf").unwrap();
        let (ok, installed, said) = piped(&sh.as_bytes()[..cut], &rel);
        assert!(!ok && !installed, "a download cut short runs nothing: {said}");
        std::fs::remove_dir_all(home.join(".cargo")).unwrap();
        let (ok, said) = install(&sha);
        assert!(!ok && said.contains("install Rust first"), "{said}");
    }

    #[test]
    fn a_release_says_where_its_files_go() {
        let files = ["dist/stable/linux-x86_64.json", "dist/stable/notes-1.1.0.tar.gz"];
        let help = |url, channel| publish_help(url, channel, "1.1.0", &files);
        assert_eq!(help(Some("https://x.example/notes"), "beta"), "upload them to https://x.example/notes/beta/");
        let latest = help(Some("github:me/notes"), "stable");
        assert!(
            latest.ends_with(
                "gh release create v1.1.0 --repo me/notes dist/stable/linux-x86_64.json dist/stable/notes-1.1.0.tar.gz"
            ),
            "{latest}"
        );
        assert!(help(Some("github:me/notes"), "beta").contains("gh release upload beta --repo me/notes --clobber"));
        assert!(help(None, "stable").contains("set [updates] url"));
    }
}

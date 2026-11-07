//! Prebuilt framework artifacts for `ax dev`'s first build (a prototype).
//!
//! A first `ax dev` on an empty build folder compiles aio-rs's dependencies
//! (and aio-rs itself, when it is a registry or git dependency) before the
//! app: ~20 s. What it compiles depends only on the toolchain, the host, ax
//! dev's profile and features, the build's flags and the locked versions,
//! never on the app. The same files can then be built once, published, and
//! fetched:
//!
//! - the key ([`key`]): SHA-256 of `rustc -vV`, ax's layout and feature, the
//!   build's setup ([`setup`]: rustflags and profile variables, cargo config
//!   files, the workspace's profiles, the host's C library) and the lock's
//!   registry and git packages (name, version, source, checksum);
//! - `<url>/<key>/SHA256SUMS` lists `<key>.zip`; `SHA256SUMS.sig` signs it
//!   (ed25519, aio's update keys). Fail closed: only an `https://` server,
//!   and only with `AIORT_PREBUILT_KEY` to check the signature against
//!   (a release of ax will pin aio-rs's own);
//! - the zip holds the units of the registry and git packages of one
//!   build, made in a build folder of its own (`deps`, `build`,
//!   `.fingerprint` of `<host>/desktop-dev`, and of `desktop-dev`, where the
//!   build's own tools are: proc macros, build scripts). A package with
//!   native build output (`links`, a library a build script links: cc,
//!   pkg-config) and every package built on it are left out: built here.
//!   No machine's paths: rustc remaps them, rustc's `.d` lists are left
//!   out, the build folder in a build script's output is a mark, and a file
//!   that still holds the home or the build folder refuses the pack.
//!
//! A fetch is checked whole before anything reaches the build folder: an
//! entry outside those folders, with `..`, a link, a file over
//! [`MAX_FILE`], or more than [`MAX_TOTAL`] in all refuses the archive.
//! It is unpacked into a folder of its own (gone after, whatever happened),
//! then each unit moved into the build folder under cargo's own lock; a unit
//! already there stays. Every file gets one modification time, so cargo
//! finds no library newer than what was built on it. Any failure (no
//! server, a wrong sum, a bad signature, over [`WITHIN`]) is said in one
//! line, and the build goes on locally. cargo stays the judge: a unit whose
//! fingerprint doesn't match this machine's is rebuilt here.
//! `AIORT_PREBUILT_URL` turns fetching on; `ax dev <app> --pack <dir>`
//! writes the files.
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What a fetched archive may write into, under `<host>/desktop-dev`.
const KINDS: [&str; 3] = ["deps", "build", ".fingerprint"];
/// A fetched `SHA256SUMS` or its signature: at most this.
const MAX_LIST: u64 = 64 << 10;
/// The archive, as downloaded: at most this.
const MAX_ZIP: u64 = 4 << 30;
/// One unpacked file: at most this.
pub const MAX_FILE: u64 = 1 << 30;
/// All of them: at most this, and [`MAX_ENTRIES`] entries.
pub const MAX_TOTAL: u64 = 8 << 30;
const MAX_ENTRIES: usize = 100_000;
/// A whole fetch (lists, archive, unpack): within this, else built here.
pub const WITHIN: Duration = Duration::from_secs(300);
/// The build folder, in a build script's output packed: the folder it is
/// unpacked into instead.
const TARGET_MARK: &str = "/@aio-prebuilt-target@";
/// A build script's files that name the build folder (cargo's own).
const NAMES_TARGET: [&str; 3] = ["output", "root-output", "stderr"];
/// Set on rustc by `--pack`: ax as `RUSTC_WRAPPER`, these flags (`\x1f`
/// between) added to each crate's call: the machine's paths remapped.
const REMAP_ENV: &str = "AX_PREBUILT_REMAP";

/// The lock's packages that come from a registry or git (pure): one line
/// each, sorted. Path packages (the app, a checkout's aio-rs) are built here.
pub fn locked_sources(lock: &str) -> String {
    let mut out = vec![];
    for block in lock.split("[[package]]").skip(1) {
        let field = |k: &str| {
            block.lines().find_map(|l| l.strip_prefix(&format!("{k} = \"")).and_then(|v| v.strip_suffix('"')))
        };
        if let (Some(name), Some(version), Some(source)) = (field("name"), field("version"), field("source")) {
            out.push(format!("{name} {version} {source} {}", field("checksum").unwrap_or("-")));
        }
    }
    out.sort();
    out.join("\n")
}

/// The archive's key (pure): everything the fetched units depend on.
pub fn key(rustc: &str, layout: &[String], feature: &str, setup: &str, lock: &str) -> String {
    let text =
        format!("aio-prebuilt 2\n{}\n{}\n{feature}\n{setup}\n{}", rustc.trim(), layout.join(" "), locked_sources(lock));
    aio::sha256::hex(&aio::sha256::Sha256::digest(text.as_bytes()))[..32].to_string()
}

/// What else a unit's bytes depend on here, as text: the rustflags and
/// profile variables, the cargo config files that apply in `dir`, the
/// profiles of the workspace at `workspace`, the host's C library.
fn setup(dir: &Path, workspace: &Path) -> String {
    let flagged = |k: &str| {
        ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_BOOTSTRAP"].contains(&k)
            || k.starts_with("CARGO_PROFILE_")
            || k.starts_with("CARGO_BUILD_") && k.ends_with("FLAGS")
            || k.starts_with("CARGO_TARGET_") && k != "CARGO_TARGET_DIR"
    };
    let mut vars: Vec<String> = std::env::vars().filter(|(k, _)| flagged(k)).map(|(k, v)| format!("{k}={v}")).collect();
    vars.sort();
    let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from);
    let folders = dir.ancestors().map(|d| d.join(".cargo")).chain(cargo_home);
    let configs = folders.flat_map(|d| ["config.toml", "config"].map(|f| d.join(f)));
    let configs: Vec<String> = configs.filter_map(|f| std::fs::read_to_string(f).ok()).collect();
    let manifest = std::fs::read_to_string(workspace.join("Cargo.toml")).unwrap_or_default();
    let profiles = toml::from_str::<toml::Table>(&manifest)
        .ok()
        .and_then(|t| t.get("profile").and_then(|p| serde_json::to_string(p).ok()));
    let libc = std::process::Command::new("getconf").arg("GNU_LIBC_VERSION").output().ok();
    let libc = libc.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    format!("{}\n{}\n{}\n{libc}", vars.join("\n"), configs.join("\n"), profiles.unwrap_or_default())
}

/// A unit's file or folder name, `<name>-<16 hex>[.ext]` (`lib` dropped),
/// as `<name with _>-<hex>`: the same for its library, its fingerprint and
/// its build script's folders (pure).
pub fn unit_key(file: &str) -> Option<String> {
    let base = file.strip_prefix("lib").filter(|_| file.contains(['.'])).unwrap_or(file);
    let stem = base.split('.').next()?;
    let (name, hash) = stem.rsplit_once('-')?;
    (hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("{}-{hash}", name.replace('-', "_")))
}

/// An archive entry's path, if it is one a fetch may write (pure):
/// `[<host>/]desktop-dev/<deps|build|.fingerprint>/…`.
pub fn allowed(entry: &str) -> bool {
    let parts: Vec<&str> = entry.split('/').collect();
    let at = usize::from(parts.first() == Some(&crate::dev::host_triple()));
    parts.len() >= at + 3
        && parts[at] == crate::dev::PROFILE
        && KINDS.contains(&parts[at + 1])
        && parts.iter().all(|p| !p.is_empty() && *p != ".." && *p != "." && !p.contains('\\') && !p.contains(':'))
}

/// This build's key: rustc's (in `dir`, its toolchain), ax's layout and
/// `feature`, the build's setup, the workspace's lock.
fn key_here(dir: &Path, workspace: &Path, feature: &str) -> Result<String, String> {
    let out = std::process::Command::new("rustc").arg("-vV").current_dir(dir).output().map_err(|e| e.to_string())?;
    let rustc = String::from_utf8_lossy(&out.stdout);
    let lock = std::fs::read_to_string(workspace.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {e}"))?;
    Ok(key(&rustc, &crate::dev::layout(), feature, &setup(dir, workspace), &lock))
}

/// The folders the units are in: `<target>/<host>/desktop-dev` (the app's
/// libraries) and `<target>/desktop-dev` (the build's own tools).
fn units(target: &Path) -> [PathBuf; 2] {
    [target.join(crate::dev::host_triple()).join(crate::dev::PROFILE), target.join(crate::dev::PROFILE)]
}

/// A folder of ax's own, removed with everything in it when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(under: &Path, what: &str) -> Result<Self, String> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let dir = under.join(format!("{what}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Where to fetch from and the key to check with (pure): `None` when
/// fetching is off (no URL); refused unless the URL is `https://` and a
/// key is given.
pub fn source(url: Option<&str>, key: Option<&str>) -> Option<Result<(String, String), String>> {
    let url = url.filter(|u| !u.is_empty())?;
    Some(match key.map(str::trim).filter(|k| !k.is_empty()) {
        _ if !url.starts_with("https://") => Err(format!("AIORT_PREBUILT_URL {url} isn't https://")),
        None => Err("no AIORT_PREBUILT_KEY to check their signature against".into()),
        Some(k) => Ok((url.trim_end_matches('/').to_string(), k.to_string())),
    })
}

/// Fetch the prebuilt units for this build into `target`, once per key:
/// what to say (nothing when fetching is off or done).
pub fn fetch(dir: &Path, workspace: &Path, target: &Path, feature: &str) -> Option<String> {
    let env = |k: &str| std::env::var(k).ok();
    let said = |e: String| Some(format!("ax dev: no prebuilt libraries ({e}): building them here"));
    let (url, trusted) = match source(env("AIORT_PREBUILT_URL").as_deref(), env("AIORT_PREBUILT_KEY").as_deref())? {
        Ok(s) => s,
        Err(e) => return said(e),
    };
    let k = match key_here(dir, workspace, feature) {
        Ok(k) => k,
        Err(e) => return said(e),
    };
    let mine = target.join("aio").join("prebuilt");
    let done = mine.join(format!("{k}.done"));
    if done.exists() {
        return None;
    }
    // A download to wait for: the splash shows.
    crate::splash::busy();
    let started = std::time::Instant::now();
    let base = format!("{url}/{k}");
    let get = move |name: &str, to: &Path, cap: u64| download(&format!("{base}/{name}"), to, cap);
    match fetch_within(get, &k, &trusted, target, WITHIN) {
        Ok(n) => {
            _ = std::fs::write(&done, url.as_bytes());
            Some(format!(
                "ax dev: {n} prebuilt files for {k} fetched and checked in {:.1} s",
                started.elapsed().as_secs_f64()
            ))
        }
        Err(e) => said(e),
    }
}

/// `url` into `to`, at most `cap` bytes: its SHA-256.
fn download(url: &str, to: &Path, cap: u64) -> Result<String, String> {
    let mut over = false;
    let got = aio_shell::runtime::download(url, to, &mut |n, _| {
        over = n > cap;
        !over
    });
    match got {
        _ if over => Err(format!("{url}: over {} MB", cap >> 20)),
        r => r.map_err(|e| e.to_string()),
    }
}

/// A downloader: (file name, where to, at most bytes) → its SHA-256.
type Get = dyn Fn(&str, &Path, u64) -> Result<String, String> + Send;

/// [`fetch_key`] given `within`: past it, the build goes on here (the
/// download, left behind, cleans up after itself when it ends).
fn fetch_within(
    get: impl Fn(&str, &Path, u64) -> Result<String, String> + Send + 'static,
    k: &str,
    trusted: &str,
    target: &Path,
    within: Duration,
) -> Result<usize, String> {
    let mine = target.join("aio").join("prebuilt");
    let stage = Scratch::new(&mine, "stage")?;
    let (tx, rx) = std::sync::mpsc::channel();
    let (k2, trusted, at, into) = (k.to_string(), trusted.to_string(), stage.0.clone(), target.to_path_buf());
    std::thread::spawn(move || {
        let staged = staged(&get, &k2, &trusted, &at, &into);
        // Past `within` nobody listens: the stage goes with the thread.
        _ = tx.send((staged, stage));
    });
    match rx.recv_timeout(within) {
        Ok((staged, stage)) => staged.and_then(|_| install(&stage.0, target)),
        Err(_) => Err(format!("not fetched within {} s", within.as_secs())),
    }
}

/// The lists, checked, then the archive, checked, unpacked into `stage`
/// for build folder `target`: how many files.
fn staged(get: &Get, k: &str, trusted: &str, stage: &Path, target: &Path) -> Result<usize, String> {
    let list = |name: &str| -> Result<String, String> {
        let to = stage.join(name);
        get(name, &to, MAX_LIST)?;
        std::fs::read_to_string(&to).map_err(|e| format!("{name}: {e}"))
    };
    let sums = list("SHA256SUMS")?;
    let sig = list("SHA256SUMS.sig").map_err(|e| format!("no signature: {e}"))?;
    if !aio::update::verify(trusted, sums.as_bytes(), sig.trim()) {
        return Err("SHA256SUMS isn't signed by AIORT_PREBUILT_KEY".into());
    }
    let zip_name = format!("{k}.zip");
    let want = aio_shell::runtime::listed_sha(&sums, &zip_name).ok_or(format!("SHA256SUMS lists no {zip_name}"))?;
    let zip = stage.join(&zip_name);
    let got = get(&zip_name, &zip, MAX_ZIP)?;
    if got != want {
        return Err(format!("{zip_name}: its SHA-256 is {got}, SHA256SUMS says {want}"));
    }
    unpack(&zip, &stage.join("units"), target, (MAX_FILE, MAX_TOTAL))
}

/// Every entry of `zip` into `into`, all checked first ([`allowed`], no
/// link), each held to `caps` (one file's, all files') as it is written;
/// the build folder's mark read as `target`.
fn unpack(zip: &Path, into: &Path, target: &Path, caps: (u64, u64)) -> Result<usize, String> {
    let f = std::fs::File::open(zip).map_err(|e| e.to_string())?;
    let mut a = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    if a.len() > MAX_ENTRIES {
        return Err(format!("the archive has {} entries: refused whole", a.len()));
    }
    for i in 0..a.len() {
        let e = a.by_index(i).map_err(|e| e.to_string())?;
        let name = e.name().map_err(|e| e.to_string())?;
        if !allowed(name.trim_end_matches('/')) || e.is_symlink() {
            return Err(format!("the archive names {name}: refused whole"));
        }
    }
    let (mut n, mut total, at) = (0, 0u64, std::time::SystemTime::now() - Duration::from_secs(1));
    for i in 0..a.len() {
        let mut e = a.by_index(i).map_err(|e| e.to_string())?;
        let name = e.name().map_err(|e| e.to_string())?.to_string();
        let to = into.join(&name);
        if e.is_dir() {
            std::fs::create_dir_all(&to).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(p) = to.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        // Counted as it is, never as its header says: one byte over refuses it.
        let mut out = std::fs::File::create(&to).map_err(|e| format!("{}: {e}", to.display()))?;
        let got = std::io::copy(&mut (&mut e).take(caps.0 + 1), &mut out).map_err(|e| format!("{name}: {e}"))?;
        total += got;
        if got > caps.0 || total > caps.1 {
            return Err(format!("the archive unpacks to more than its cap at {name}: refused whole"));
        }
        if name.contains("/build/") && to.file_name().is_some_and(|f| NAMES_TARGET.iter().any(|t| f == *t)) {
            let text = std::fs::read_to_string(&to).map_err(|e| format!("{name}: {e}"))?;
            std::fs::write(&to, text.replace(TARGET_MARK, &target.display().to_string())).map_err(|e| e.to_string())?;
        }
        _ = out.set_modified(at);
        #[cfg(unix)]
        if let Some(mode) = e.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            _ = std::fs::set_permissions(&to, std::fs::Permissions::from_mode(mode & 0o755));
        }
        n += 1;
    }
    Ok(n)
}

/// The staged units (`stage/units`) moved into `target`, each whole, under
/// cargo's lock of each folder (no build runs meanwhile); a unit already
/// there is this machine's and stays: how many files moved.
fn install(stage: &Path, target: &Path) -> Result<usize, String> {
    let mut n = 0;
    for (from, to) in units(&stage.join("units")).iter().zip(units(target)) {
        if !from.is_dir() {
            continue;
        }
        let lock = to.join(".cargo-lock");
        std::fs::create_dir_all(&to).map_err(|e| format!("{}: {e}", to.display()))?;
        let held = std::fs::File::options().create(true).truncate(false).write(true).open(&lock);
        let held = held.map_err(|e| format!("{}: {e}", lock.display()))?;
        held.lock().map_err(|e| format!("{}: {e}", lock.display()))?;
        for kind in KINDS {
            let Ok(entries) = std::fs::read_dir(from.join(kind)) else { continue };
            std::fs::create_dir_all(to.join(kind)).map_err(|e| e.to_string())?;
            for e in entries.flatten() {
                let dest = to.join(kind).join(e.file_name());
                if dest.exists() {
                    continue;
                }
                let files = walk(&e.path()).len();
                std::fs::rename(e.path(), &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
                n += files;
            }
        }
    }
    Ok(n)
}

/// As `RUSTC_WRAPPER` for `--pack`: rustc with this machine's paths
/// remapped ([`REMAP_ENV`]). `None`: ax isn't that.
pub fn hook() -> Option<std::process::ExitCode> {
    let flags = std::env::var(REMAP_ENV).ok()?;
    let mut args = std::env::args_os().skip(1);
    let rustc = args.next()?;
    let args: Vec<std::ffi::OsString> = args.collect();
    let mut cmd = std::process::Command::new(&rustc);
    cmd.args(&args).env_remove(REMAP_ENV);
    // A crate's compile, not cargo asking rustc what it is.
    if args.iter().any(|a| a == "--crate-name") {
        cmd.args(flags.split('\x1f').filter(|f| !f.is_empty()));
    }
    let e = crate::hotpatch::become_(cmd);
    eprintln!("ax: can't run {}: {e}", rustc.to_string_lossy());
    Some(std::process::ExitCode::FAILURE)
}

/// `ax dev <app> --pack <out>`: package `pkg` built in a build folder of
/// its own, and the units of its registry and git packages from it into
/// `<out>/<key>/<key>.zip`, `SHA256SUMS` (and `SHA256SUMS.sig` with
/// `AIORT_PREBUILT_SIGN` naming a PKCS#8 key file).
pub fn pack(pkg: &str, dir: &Path, workspace: &Path, feature: &str, out: &Path) -> Result<String, String> {
    let k = key_here(dir, workspace, feature)?;
    let full = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--locked"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    let meta: serde_json::Value = serde_json::from_slice(&full.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let fresh = Scratch::new(&std::path::absolute(out).map_err(|e| e.to_string())?, ".target")?;
    let home = std::env::home_dir().filter(|h| h.components().count() > 1);
    let cargo_home =
        std::env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| home.as_ref().map(|h| h.join(".cargo")));
    let remaps = crate::build::remaps(home.as_deref(), cargo_home.as_deref(), &fresh.0);
    let mut cmd = std::process::Command::new("cargo");
    cmd.args(["build", "-p", pkg, "--message-format=json-render-diagnostics", "--locked"])
        .args(crate::dev::layout())
        .args(["--features", feature])
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", &fresh.0)
        .env("RUSTC_WRAPPER", std::env::current_exe().map_err(|e| e.to_string())?)
        .env(REMAP_ENV, remaps.join("\x1f"));
    let ran = crate::hotpatch::cargo_out::run(&mut cmd, true, &Default::default()).map_err(|e| e.to_string())?;
    if !ran.status.success() {
        return Err(format!("the build to pack failed:\n{}", ran.said.trim_end()));
    }
    let keep = packed_units(&meta, &ran.stdout);
    let mut needles: Vec<String> = [home, cargo_home].into_iter().flatten().map(|p| p.display().to_string()).collect();
    needles.push(fresh.0.display().to_string());
    let at = out.join(&k);
    std::fs::create_dir_all(&at).map_err(|e| e.to_string())?;
    let zip_path = at.join(format!("{k}.zip"));
    let n = write_zip(&fresh.0, &keep, &needles, &zip_path)?;
    let bytes = std::fs::read(&zip_path).map_err(|e| e.to_string())?;
    let sha = aio::sha256::hex(&aio::sha256::Sha256::digest(&bytes));
    let sums = format!("{sha}  {k}.zip\n");
    std::fs::write(at.join("SHA256SUMS"), &sums).map_err(|e| e.to_string())?;
    if let Some(keyfile) = std::env::var_os("AIORT_PREBUILT_SIGN") {
        // A key file as `ax keygen --aio` writes it.
        let pkcs8 = crate::release::read_key(Path::new(&keyfile)).map_err(|e| format!("AIORT_PREBUILT_SIGN: {e}"))?;
        std::fs::write(at.join("SHA256SUMS.sig"), aio::update::sign_with(&pkcs8, sums.as_bytes())? + "\n")
            .map_err(|e| e.to_string())?;
    }
    Ok(format!("prebuilt {k}: {n} files, {} MB, in {}", bytes.len() >> 20, at.display()))
}

/// The units to pack (pure): those the build's messages name (its
/// libraries, proc macros, build scripts and their runs) of registry and
/// git packages, but for a package with native build output (its
/// `links`, a library its build script links) and every package built on it.
pub fn packed_units(meta: &serde_json::Value, messages: &str) -> HashSet<String> {
    let msgs: Vec<serde_json::Value> = messages.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let pkgs = meta["packages"].as_array().into_iter().flatten();
    let mut out: HashSet<&str> =
        pkgs.filter(|p| p["source"].is_null() || p["links"].is_string()).filter_map(|p| p["id"].as_str()).collect();
    let linked = |m: &serde_json::Value| {
        ["linked_libs", "linked_paths"].iter().any(|k| m[k].as_array().is_some_and(|a| !a.is_empty()))
    };
    out.extend(
        msgs.iter()
            .filter(|m| m["reason"] == "build-script-executed" && linked(m))
            .filter_map(|m| m["package_id"].as_str()),
    );
    // And every package built on one of them, until none is added.
    let nodes: Vec<&serde_json::Value> = meta["resolve"]["nodes"].as_array().into_iter().flatten().collect();
    loop {
        let more: Vec<&str> = nodes
            .iter()
            .filter(|n| n["id"].as_str().is_some_and(|id| !out.contains(id)))
            .filter(|n| {
                n["dependencies"].as_array().into_iter().flatten().any(|d| d.as_str().is_some_and(|d| out.contains(d)))
            })
            .filter_map(|n| n["id"].as_str())
            .collect();
        if more.is_empty() {
            break;
        }
        out.extend(more);
    }
    let mut keep = HashSet::new();
    for m in msgs.iter().filter(|m| m["package_id"].as_str().is_some_and(|id| !out.contains(id))) {
        let files = m["filenames"].as_array().into_iter().flatten().filter_map(|f| f.as_str());
        let dirs = m["out_dir"].as_str().into_iter();
        for f in files.chain(dirs).map(Path::new) {
            // A build script's files are named by their unit's folder.
            let unit = match f.parent().and_then(Path::parent).and_then(Path::file_name) {
                Some(kind) if kind == "build" => f.parent().and_then(Path::file_name),
                _ => f.file_name(),
            };
            keep.extend(unit.and_then(|u| unit_key(&u.to_string_lossy())));
        }
    }
    keep
}

/// The files of units `keep` in build folder `fresh` into the zip at `to`:
/// how many. No `.d` file (rustc's list of sources, by their paths here);
/// the build folder in a build script's output as [`TARGET_MARK`]; a file
/// still holding one of `needles` (the home, the build folder) refuses it.
fn write_zip(fresh: &Path, keep: &HashSet<String>, needles: &[String], to: &Path) -> Result<usize, String> {
    let file = std::fs::File::create(to).map_err(|e| e.to_string())?;
    let mut z = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut n = 0;
    for (root, kind) in units(fresh).iter().flat_map(|r| KINDS.map(|k| (r.clone(), k))) {
        let Ok(entries) = std::fs::read_dir(root.join(kind)) else { continue };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            let name = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            if !unit_key(&name).is_some_and(|u| keep.contains(&u)) {
                continue;
            }
            for f in walk(&p).into_iter().filter(|f| f.extension().is_none_or(|x| x != "d")) {
                let rel = f.strip_prefix(fresh).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
                let mut bytes = std::fs::read(&f).map_err(|e| format!("{}: {e}", f.display()))?;
                if kind == "build" && f.file_name().is_some_and(|x| NAMES_TARGET.iter().any(|t| x == *t)) {
                    let text = String::from_utf8_lossy(&bytes).replace(&fresh.display().to_string(), TARGET_MARK);
                    bytes = text.into_bytes();
                }
                if let Some(found) = needles.iter().find(|s| bytes.windows(s.len()).any(|w| w == s.as_bytes())) {
                    return Err(format!("{rel} holds this machine's path {found}: not packed"));
                }
                z.start_file(rel, opts.unix_permissions(file_mode(&f))).map_err(|e| e.to_string())?;
                std::io::Write::write_all(&mut z, &bytes).map_err(|e| e.to_string())?;
                n += 1;
            }
        }
    }
    z.finish().map_err(|e| e.to_string())?;
    Ok(n)
}

fn file_mode(f: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(f).map_or(0o644, |m| m.permissions().mode() & 0o755)
    }
    #[cfg(not(unix))]
    {
        _ = f;
        0o644
    }
}

fn walk(p: &Path) -> Vec<PathBuf> {
    if p.is_file() {
        return vec![p.to_path_buf()];
    }
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(p) {
        let mut kids: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        kids.sort();
        for k in kids {
            out.extend(walk(&k));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = "version = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"syn\"\nversion = \"2.0.1\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"ab\"\n\n[[package]]\nname = \"aio-rs\"\nversion = \"0.3.0\"\nsource = \"git+https://example/aio-rs?tag=v0.3.0#123\"\n";

    #[test]
    fn the_key_is_the_toolchain_the_layout_the_setup_and_the_locked_sources_not_the_app() {
        assert_eq!(
            locked_sources(LOCK),
            "aio-rs 0.3.0 git+https://example/aio-rs?tag=v0.3.0#123 -\nsyn 2.0.1 registry+https://github.com/rust-lang/crates.io-index ab"
        );
        let l = vec!["--profile".to_string(), "desktop-dev".into()];
        let k = key("rustc 1.98.0", &l, "aio-shell/dev", "", LOCK);
        assert_eq!(k.len(), 32);
        let other_app = LOCK.replace("name = \"app\"", "name = \"notes\"");
        assert_eq!(key("rustc 1.98.0", &l, "aio-shell/dev", "", &other_app), k, "another app, the same libraries");
        assert_ne!(key("rustc 1.99.0", &l, "aio-shell/dev", "", LOCK), k);
        assert_ne!(key("rustc 1.98.0", &l, "aio-shell/dev", "", &LOCK.replace("2.0.1", "2.0.2")), k);
        assert_ne!(key("rustc 1.98.0", &l, "aio-shell/dev", "RUSTFLAGS=-Ctarget-cpu=native", LOCK), k, "flags");
        assert_ne!(key("rustc 1.98.0", &l, "aio-shell/dev", "glibc 2.31", LOCK), k, "another C library");
    }

    #[test]
    fn the_setup_has_the_flags_the_cargo_config_and_the_profiles() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("ws");
        std::fs::create_dir_all(ws.join(".cargo")).unwrap();
        std::fs::write(ws.join("Cargo.toml"), "[workspace]\n[profile.dev.package.\"*\"]\nopt-level = 2\n").unwrap();
        std::fs::write(ws.join(".cargo/config.toml"), "[build]\nrustflags = [\"-Cfoo\"]\n").unwrap();
        let s = setup(&ws, &ws);
        assert!(s.contains("rustflags = [\"-Cfoo\"]") && s.contains("opt-level"), "{s}");
    }

    #[test]
    fn a_unit_is_named_by_its_package_and_hash_and_an_entry_stays_in_the_build_folder() {
        assert_eq!(unit_key("libsyn-0123456789abcdef.rlib").as_deref(), Some("syn-0123456789abcdef"));
        assert_eq!(unit_key("serde_derive-0123456789abcdef.d").as_deref(), Some("serde_derive-0123456789abcdef"));
        assert_eq!(unit_key("aio-rs-shell-0123456789abcdef").as_deref(), Some("aio_rs_shell-0123456789abcdef"));
        assert_eq!(unit_key("libc-0123456789abcdef").as_deref(), Some("libc-0123456789abcdef"), "a folder");
        assert_eq!(unit_key("notes"), None);
        let host = crate::dev::host_triple();
        assert!(allowed(&format!("{host}/desktop-dev/deps/libsyn-0123456789abcdef.rlib")));
        assert!(allowed("desktop-dev/.fingerprint/syn-0123456789abcdef/lib-syn"), "the build's tools");
        let up = format!("{host}/desktop-dev/deps/../../x");
        for bad in [
            "../x",
            &up,
            "/etc/passwd",
            "desktop-dev/bin/x",
            "desktop-dev/deps",
            "desktop-dev/deps//x",
            "other/deps/x",
            "deps/x",
        ] {
            assert!(!allowed(bad), "{bad}");
        }
    }

    #[test]
    fn only_https_with_a_key_is_fetched() {
        assert_eq!(source(None, Some("ed25519:x")), None, "off");
        assert_eq!(source(Some(""), None), None, "off");
        assert!(source(Some("http://example.com"), Some("ed25519:x")).unwrap().unwrap_err().contains("https"));
        assert!(source(Some("https://example.com"), None).unwrap().unwrap_err().contains("AIORT_PREBUILT_KEY"));
        assert!(source(Some("https://example.com"), Some(" ")).unwrap().is_err(), "an empty key is none");
        assert_eq!(
            source(Some("https://example.com/p/"), Some("ed25519:x")),
            Some(Ok(("https://example.com/p".into(), "ed25519:x".into())))
        );
    }

    const H: &str = "0123456789abcdef";

    /// A server's folder for key `k`: the zip of `entries`, its sums, signed by `pkcs8`.
    fn served(dir: &Path, k: &str, entries: &[(&str, &[u8])], pkcs8: &[u8]) {
        std::fs::create_dir_all(dir).unwrap();
        let mut z = zip::ZipWriter::new(std::fs::File::create(dir.join(format!("{k}.zip"))).unwrap());
        for (name, bytes) in entries {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut z, bytes).unwrap();
        }
        z.finish().unwrap();
        let sha = aio::sha256::hex(&aio::sha256::Sha256::digest(std::fs::read(dir.join(format!("{k}.zip"))).unwrap()));
        let sums = format!("{sha}  {k}.zip\n");
        std::fs::write(dir.join("SHA256SUMS"), &sums).unwrap();
        std::fs::write(dir.join("SHA256SUMS.sig"), aio::update::sign_with(pkcs8, sums.as_bytes()).unwrap()).unwrap();
    }

    /// A downloader of the files in `dir`, held to its caps.
    fn from(dir: &Path) -> impl Fn(&str, &Path, u64) -> Result<String, String> + Send + 'static {
        let dir = dir.to_path_buf();
        move |name: &str, to: &Path, cap: u64| {
            let bytes = std::fs::read(dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
            if bytes.len() as u64 > cap {
                return Err(format!("{name}: over the cap"));
            }
            std::fs::write(to, &bytes).map_err(|e| e.to_string())?;
            Ok(aio::sha256::hex(&aio::sha256::Sha256::digest(&bytes)))
        }
    }

    /// What is left of fetches in `target`'s own folder (stages are gone).
    fn left(target: &Path) -> Vec<String> {
        let dir = target.join("aio/prebuilt");
        std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into()).collect()
    }

    #[test]
    fn a_fetch_lands_whole_or_not_at_all_and_leaves_nothing_behind() {
        let d = tempfile::tempdir().unwrap();
        let (server, target) = (d.path().join("server"), d.path().join("target"));
        let (pkcs8, public) = aio::update::keygen().unwrap();
        let host = crate::dev::host_triple();
        let lib = format!("{host}/desktop-dev/deps/libsyn-{H}.rlib");
        let fp = format!("{host}/desktop-dev/.fingerprint/syn-{H}/lib-syn");
        let out = format!("desktop-dev/build/serde-{H}/root-output");
        let mark = format!("{TARGET_MARK}/desktop-dev/build/serde-{H}/out");
        let good: [(&str, &[u8]); 3] = [(&lib, b"rlib"), (&fp, b"fp"), (&out, mark.as_bytes())];
        let fetch = |dir: &Path, key: &str| fetch_within(from(dir), "k", key, &target, WITHIN);
        // Refused: an entry outside the build's folders, a list not signed by the key, a wrong sum.
        served(&server.join("bad"), "k", &[(&lib, b"rlib"), ("desktop-dev/bin/evil", b"x")], &pkcs8);
        assert!(fetch(&server.join("bad"), &public).unwrap_err().contains("refused whole"));
        served(&server.join("good"), "k", &good, &pkcs8);
        let (_, other) = aio::update::keygen().unwrap();
        assert!(fetch(&server.join("good"), &other).unwrap_err().contains("isn't signed"));
        std::fs::write(server.join("good/SHA256SUMS.sig"), "AAAA").unwrap();
        assert!(fetch(&server.join("good"), &public).unwrap_err().contains("isn't signed"), "a bad signature");
        served(&server.join("sum"), "k", &good, &pkcs8);
        std::fs::write(server.join("sum/k.zip"), b"not the zip it lists").unwrap();
        assert!(fetch(&server.join("sum"), &public).unwrap_err().contains("SHA256SUMS says"));
        assert!(!target.join(&lib).exists(), "nothing of a refused fetch reached the build folder");
        assert!(left(&target).is_empty(), "no stage left: {:?}", left(&target));
        // Taken: each unit moved in, the build folder's mark read as this one, nothing left over.
        served(&server.join("good"), "k", &good, &pkcs8);
        assert_eq!(fetch(&server.join("good"), &public).unwrap(), 3);
        assert_eq!(std::fs::read(target.join(&lib)).unwrap(), b"rlib");
        let root_output = std::fs::read_to_string(target.join(&out)).unwrap();
        assert_eq!(root_output, format!("{}/desktop-dev/build/serde-{H}/out", target.display()));
        assert!(left(&target).is_empty(), "{:?}", left(&target));
        // A unit already here is this machine's: it stays.
        std::fs::write(target.join(&lib), b"built here").unwrap();
        fetch(&server.join("good"), &public).unwrap();
        assert_eq!(std::fs::read(target.join(&lib)).unwrap(), b"built here");
    }

    #[test]
    fn a_fetch_past_its_time_or_its_size_builds_here() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("target");
        let slow = |_: &str, _: &Path, _: u64| -> Result<String, String> {
            std::thread::sleep(Duration::from_millis(500));
            Err("late".into())
        };
        let late = fetch_within(slow, "k", "ed25519:x", &target, Duration::from_millis(50));
        assert!(late.unwrap_err().contains("within"));
        std::thread::sleep(Duration::from_millis(700));
        assert!(left(&target).is_empty(), "its stage went with it: {:?}", left(&target));
        // A zip bomb: a file that unpacks past the cap, whatever its header says.
        let zip = d.path().join("bomb.zip");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&zip).unwrap());
        let host = crate::dev::host_triple();
        z.start_file(format!("{host}/desktop-dev/deps/libx-{H}.rlib"), zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut z, &vec![0u8; 3 << 20]).unwrap();
        z.finish().unwrap();
        assert!(std::fs::metadata(&zip).unwrap().len() < 64 << 10, "small as it comes");
        let units = d.path().join("units");
        assert!(unpack(&zip, &units, &target, (2 << 20, 8 << 20)).unwrap_err().contains("refused whole"), "one file");
        assert!(unpack(&zip, &units, &target, (8 << 20, 2 << 20)).unwrap_err().contains("refused whole"), "in all");
        assert_eq!(unpack(&zip, &units, &target, (8 << 20, 8 << 20)).unwrap(), 1, "under both caps");
    }

    #[test]
    fn a_pack_has_this_builds_units_alone_and_no_machines_paths() {
        let d = tempfile::tempdir().unwrap();
        let fresh = d.path().join("fresh");
        let host = crate::dev::host_triple();
        let deps = fresh.join(host).join("desktop-dev/deps");
        let build = fresh.join("desktop-dev/build");
        std::fs::create_dir_all(&deps).unwrap();
        std::fs::create_dir_all(build.join(format!("serde-{H}"))).unwrap();
        std::fs::write(deps.join(format!("libsyn-{H}.rlib")), b"rlib").unwrap();
        std::fs::write(deps.join(format!("syn-{H}.d")), b"/h0me/someone/.cargo/registry/src/syn/lib.rs").unwrap();
        // A unit of another app, in the same folder: not this build's.
        std::fs::write(deps.join("libforeign-fedcba9876543210.rlib"), b"other").unwrap();
        let out = fresh.join(format!("desktop-dev/build/serde-{H}/out"));
        std::fs::write(build.join(format!("serde-{H}/root-output")), out.display().to_string()).unwrap();
        let keep: HashSet<String> = [format!("syn-{H}"), format!("serde-{H}")].into();
        let needles = ["/h0me/someone".to_string(), fresh.display().to_string()];
        let zip = d.path().join("p.zip");
        assert_eq!(write_zip(&fresh, &keep, &needles, &zip).unwrap(), 2);
        let mut a = zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap();
        let names: Vec<String> = a.file_names().map(|n| n.unwrap().to_string()).collect();
        assert!(names.iter().all(|n| !n.contains("foreign") && !n.ends_with(".d")), "{names:?}");
        let mut root = String::new();
        a.by_name(&format!("desktop-dev/build/serde-{H}/root-output")).unwrap().read_to_string(&mut root).unwrap();
        let rest = out.strip_prefix(&fresh).unwrap();
        assert_eq!(
            root,
            format!("{TARGET_MARK}{}{}", std::path::MAIN_SEPARATOR, rest.display()),
            "the folder as a mark"
        );
        // A library that still names the home: the pack is refused.
        std::fs::write(deps.join(format!("libsyn-{H}.rlib")), b"rlib /h0me/someone/.cargo").unwrap();
        assert!(write_zip(&fresh, &keep, &needles, &zip).unwrap_err().contains("/h0me/someone"));
    }

    #[test]
    fn a_pack_leaves_out_path_and_native_packages_and_all_built_on_them() {
        let meta = serde_json::json!({
            "packages": [
                { "id": "app", "source": null },
                { "id": "syn", "source": "registry" },
                { "id": "cc-lib", "source": "registry" },
                { "id": "sys", "source": "registry", "links": "z" },
                { "id": "on-sys", "source": "registry" },
                { "id": "on-cc", "source": "registry" },
            ],
            "resolve": { "nodes": [
                { "id": "on-sys", "dependencies": ["sys"] },
                { "id": "on-cc", "dependencies": ["cc-lib", "syn"] },
                { "id": "syn", "dependencies": [] },
            ]},
        });
        let unit = |id: &str, f: String| {
            serde_json::json!({ "reason": "compiler-artifact", "package_id": id, "filenames": [f] }).to_string()
        };
        let messages = [
            unit("syn", format!("/t/deps/libsyn-{H}.rlib")),
            unit("app", format!("/t/deps/libapp-{H}.rlib")),
            unit("sys", format!("/t/deps/libsys-{H}.rlib")),
            unit("on-sys", format!("/t/deps/libon_sys-{H}.rlib")),
            unit("on-cc", format!("/t/deps/libon_cc-{H}.rlib")),
            unit("syn", format!("/t/build/syn-{H}/build-script-build")),
            serde_json::json!({ "reason": "build-script-executed", "package_id": "cc-lib",
                "linked_libs": ["static=x"], "linked_paths": [], "out_dir": format!("/t/build/cc-lib-{H}/out") })
            .to_string(),
        ]
        .join("\n");
        let keep = packed_units(&meta, &messages);
        assert_eq!(keep, [format!("syn-{H}")].into_iter().collect::<HashSet<_>>());
    }
}

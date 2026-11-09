//! The shared Electron runtime, in aio's store (`shared-runtime.ts`,
//! `electron-runtime-fetch.ts`): one per version per machine, the same copy
//! aio's apps start from. Installed as aio installs it: under aio's lock
//! (`<entry>.lock`, [`crate::pidlock`]), downloaded and checked against the
//! SHA-256 the build pinned, unpacked into `<entry>.incoming.<tag>`, fused,
//! stamped `.aio-complete` and renamed into place, so a half-done download
//! is never used, by either.
use aio::sha256::Sha256;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The pinned version, the same as aio's (proven on the same apps).
pub const VERSION: &str = aio::electron_version!();

/// The SHA-256 of each platform's zip of [`VERSION`] (`<slug>=<hex>`), as
/// aio's online build pins them from the release's `SHASUMS256.txt`.
macro_rules! electron_sha256 {
    () => {
        "darwin-arm64=4af51382278d00291dfcda265eddd9bab6e98cebb09f46aadc8438bf606de162 \
         darwin-x64=451363b59bb7d2c1c9e0c4baa99106004eaabe724aa21a0c6b4c87469293531a \
         linux-arm64=8e70d53d920d84db869023280e5a1fbda82ecc58fea4443748e58309a3c80238 \
         linux-x64=6e7fd2d450d442507b846ace4ae6e2bd17e1b1653e86a033b5231abfc23e6df4 \
         win32-arm64=66662802d00c2626a203f8909467dab3504def4ce8615c152135b994a5cec220 \
         win32-x64=41a1ffa0541cb0f2c9b9fe920dd2cef1793ad0ea2ada1a9582958880f4d1b23c"
    };
}

/// [`VERSION`]'s pins, in the binary's bytes too ([`sha_in`]): an update
/// checks the runtime a new version needs with that version's own pins.
#[used]
static PINNED_SHAS: &[u8] = concat!("\0aio-electron-sha256=", electron_sha256!(), "\0").as_bytes();

/// The pin of `slug` in a list of `<slug>=<hex>` (pure).
pub fn sha_for(list: &str, slug: &str) -> Option<String> {
    list.split_whitespace().find_map(|p| {
        let (s, hex) = p.split_once('=')?;
        (s == slug && hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| hex.to_lowercase())
    })
}

/// This build's pin of `version` for this machine: none for another version.
pub fn pinned_sha(version: &str) -> Option<String> {
    std::hint::black_box(PINNED_SHAS);
    (version == VERSION).then(|| sha_for(electron_sha256!(), &slug())).flatten()
}

/// The pin of the runtime `version` a binary's bytes carry for this
/// machine: its own list, else this build's when it is [`VERSION`] (a
/// build before the list was written).
pub fn sha_in(binary: &[u8], version: &str) -> Option<String> {
    let tag = b"\0aio-electron-sha256=";
    let list = binary.windows(tag.len()).enumerate().filter(|(_, w)| w == tag).find_map(|(at, _)| {
        let at = at + tag.len();
        let len = binary[at..].iter().take(1024).position(|b| *b == 0).filter(|n| *n > 0)?;
        std::str::from_utf8(&binary[at..at + len]).ok()
    });
    list.and_then(|l| sha_for(l, &slug())).or_else(|| pinned_sha(version))
}

/// Written last into a complete runtime: `<url>\nsha256:<hex>\n` (aio's).
pub const COMPLETE: &str = ".aio-complete";

/// Is the runtime for `os` in `dir` complete, its executable there? (aio's
/// `runtimeUsable`.)
pub fn usable(os: &str, dir: &Path) -> bool {
    dir.join(COMPLETE).exists() && std::fs::metadata(exe_for(os, dir)).is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// How long a start waits for another process's download of the same runtime.
const LOCK_WAIT: Duration = Duration::from_secs(15 * 60);

/// The pinned version, findable in the binary's bytes ([`pinned_in`]): an
/// update fetches the runtime a new version needs before it swaps to it,
/// without running the new binary.
#[used]
static PINNED: &[u8] = concat!("\0aio-electron-runtime=", aio::electron_version!(), "\0").as_bytes();

/// The runtime version a binary's bytes pin, if it says (one built before
/// this was written doesn't).
pub fn pinned_in(binary: &[u8]) -> Option<String> {
    // The tag itself is in the binary too (this function's), with no
    // version after it: the first one that has one.
    let tag = b"\0aio-electron-runtime=";
    binary.windows(tag.len()).enumerate().filter(|(_, w)| w == tag).find_map(|(at, _)| {
        let at = at + tag.len();
        let len = binary[at..].iter().take(32).position(|b| *b == 0)?;
        let v = std::str::from_utf8(&binary[at..at + len]).ok()?;
        aio::update::parse_version(v).map(|_| v.to_string())
    })
}

/// Get Electron `version` into the store, quietly (no window), unless it is
/// there: what a staged update needs before the swap (its first start
/// must not wait on a download), and what `ax setup` fetches (a terminal
/// shows the progress; no window lands on anyone's screen). Returns its
/// store entry.
/// `sha` is the zip's pin ([`pinned_sha`], [`sha_in`]); none is refused.
pub fn prefetch(version: &str, sha: Option<&str>, step: &dyn Fn(u8)) -> io::Result<PathBuf> {
    migrate();
    prefetch_in(&root(), version, sha, &mirror(), step)
}

fn prefetch_in(root: &Path, version: &str, sha: Option<&str>, mirror: &str, step: &dyn Fn(u8)) -> io::Result<PathBuf> {
    let dir = root.join(format!("{version}-{}", slug()));
    if !usable(std::env::consts::OS, &dir) {
        eprintln!("[aio] getting the Electron {version} runtime (once per machine)…");
        let colors = crate::splash::colors(aio::colors::hue("aio"), None);
        let look = crate::firstrun::Look { colors: &colors, at: None, icon: None, image: None };
        fetch(&dir, None, "", version, sha, mirror, &look, step).map_err(|(e, _)| e)?;
    }
    stamp(&dir);
    Ok(dir)
}

/// Electron's name for this OS and CPU.
pub fn slug() -> String {
    aio::paths::electron_slug()
}

/// The shared store, aio's: one entry per version (`crate::prune`).
pub fn root() -> PathBuf {
    aio::paths::electron_store()
}

/// Where this version lives: `AIORT_RUNTIME`, else the one bundled beside
/// the app's binary ([`BUNDLED`]), else the store.
pub fn dir() -> PathBuf {
    match std::env::var_os("AIORT_RUNTIME") {
        Some(p) if !p.is_empty() => p.into(),
        _ => bundled().unwrap_or_else(|| root().join(format!("{VERSION}-{}", slug()))),
    }
}

/// The folder beside the app's binary that holds its own runtime, in an
/// offline build (`[build] runtime = "bundled"`, [`bundle`]).
pub const BUNDLED: &str = "electron";

/// The runtime bundled with this binary, if it has one.
fn bundled() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.join(BUNDLED);
    exe(&dir).exists().then_some(dir)
}

/// Does the release unpacked in `dir` carry its own runtime?
pub fn carries_runtime(dir: &Path) -> bool {
    exe(&dir.join(BUNDLED)).exists()
}

/// The locales an offline build keeps (aio's: Chromium names them with
/// regions); the app's own text is not in them.
pub const KEPT_LOCALES: [&str; 2] = ["en-US", "en-GB"];

/// Chromium's optional GPU extras (aio's list): software Vulkan and the
/// DirectX shader compilers. Without them a machine with no working GPU
/// driver draws less well; `[build] chromium_extras = "strip"` drops them.
pub const CHROMIUM_EXTRAS: [&str; 7] = [
    "dxcompiler.dll",
    "dxil.dll",
    "vk_swiftshader.dll",
    "vk_swiftshader_icd.json",
    "libvk_swiftshader.so",
    "vulkan-1.dll",
    "libvulkan.so.1",
];

/// The locales a macOS offline build keeps, as its `.lproj` folders name
/// them (aio's `electron-locales.ts`).
pub const KEPT_LPROJ: [&str; 2] = ["en", "en_GB"];

/// Where a macOS runtime keeps its `.lproj` locales: the bundle's and the
/// framework's resources.
const MAC_LOCALES: [&str; 2] = [
    "Electron.app/Contents/Resources",
    "Electron.app/Contents/Frameworks/Electron Framework.framework/Versions/A/Resources",
];

/// Trim the runtime for `os` unpacked in `dir` for an offline build: every
/// locale but [`KEPT_LOCALES`] (macOS: [`KEPT_LPROJ`]), and (`extras`) the
/// [`CHROMIUM_EXTRAS`]. What went, by name.
pub fn trim(os: &str, dir: &Path, extras: bool) -> io::Result<Vec<String>> {
    let mut gone = vec![];
    let (places, suffix, kept): (&[&str], _, &[&str]) = match os {
        "macos" => (&MAC_LOCALES, ".lproj", &KEPT_LPROJ),
        _ => (&["locales"], ".pak", &KEPT_LOCALES),
    };
    for place in places {
        let entries = match std::fs::read_dir(dir.join(place)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            r => r?,
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.strip_suffix(suffix).is_some_and(|l| !kept.contains(&l)) {
                match e.file_type()?.is_dir() {
                    true => std::fs::remove_dir_all(e.path())?,
                    false => std::fs::remove_file(e.path())?,
                }
                gone.push(format!("{place}/{name}"));
            }
        }
    }
    for x in CHROMIUM_EXTRAS.iter().filter(|_| extras) {
        match std::fs::remove_file(dir.join(x)) {
            Ok(()) => gone.push(x.to_string()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(gone)
}

/// Electron [`VERSION`] for `slug` (`linux-arm64`), in `cache` once and
/// checked against its SHA-256 list there every time: a rebuild needs no
/// network.
fn cached_zip(cache: &Path, slug: &str, mirror: &str) -> io::Result<PathBuf> {
    let dir = cache.join(format!("v{VERSION}"));
    std::fs::create_dir_all(&dir)?;
    let base = format!("{}/v{VERSION}/", mirror.trim_end_matches('/'));
    let (file, sums_file) = (format!("electron-v{VERSION}-{slug}.zip"), dir.join("SHASUMS256.txt"));
    let sums = match std::fs::read_to_string(&sums_file) {
        Ok(s) if listed_sha(&s, &file).is_some() => s,
        _ => {
            let s = get_text(&format!("{base}SHASUMS256.txt"))?;
            aio::paths::write_atomic(&sums_file, s.as_bytes(), false)?;
            s
        }
    };
    let want = listed_sha(&sums, &file)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("{file} is not in SHASUMS256.txt")))?;
    let zip = dir.join(&file);
    let sha = |p: &Path| -> io::Result<String> {
        let mut h = Sha256::new();
        io::copy(&mut std::fs::File::open(p)?, &mut h)?;
        Ok(aio::sha256::hex(&h.finalize()))
    };
    if zip.exists() && sha(&zip)? == want {
        return Ok(zip);
    }
    eprintln!("[aio] getting Electron {VERSION} for {slug} to bundle (once)…");
    let tmp = dir.join(format!(".{file}.partial-{}", std::process::id()));
    let got = download(&format!("{base}{file}"), &tmp, &mut |_, _| true);
    let r = match got {
        Ok(got) if got == want => std::fs::rename(&tmp, &zip),
        Ok(got) => Err(io::Error::new(io::ErrorKind::InvalidData, format!("{file}: SHA-256 {got}, expected {want}"))),
        Err(e) => Err(e),
    };
    _ = std::fs::remove_file(&tmp);
    r.map(|()| zip)
}

/// Put Electron [`VERSION`] for `slug` into `into` (a bundle's
/// [`BUNDLED`] folder) for an offline build: from `cache`
/// (`~/.cache/aio-rt/electron`), trimmed ([`trim`]) and fused; a macOS one
/// signed again, ad hoc, so made on a Mac. What was trimmed, said.
pub fn bundle(into: &Path, slug: &str, cache: &Path, extras: bool) -> io::Result<Vec<String>> {
    bundle_from(into, slug, cache, extras, &mirror())
}

fn bundle_from(into: &Path, slug: &str, cache: &Path, extras: bool, mirror: &str) -> io::Result<Vec<String>> {
    let os = os_of(slug);
    if os == "macos" && std::env::consts::OS != "macos" {
        return Err(io::Error::other("a trimmed macOS runtime is signed again, which takes a Mac: build it there"));
    }
    let zip = cached_zip(cache, slug, mirror)?;
    _ = std::fs::remove_dir_all(into);
    install_for(os, std::fs::File::open(zip)?, into)?;
    let gone = trim(os, into, extras)?;
    fuses_off(os, into)?;
    if os == "macos" {
        codesign(&into.join("Electron.app"), true)?;
    }
    std::fs::write(into.join(FUSED), "")?;
    Ok(gone)
}

/// The runtime's executable inside `dir`.
pub fn exe(dir: &Path) -> PathBuf {
    exe_for(std::env::consts::OS, dir)
}

/// The executable inside `dir` of a runtime for `os`
/// (`std::env::consts::OS`'s word).
pub fn exe_for(os: &str, dir: &Path) -> PathBuf {
    aio::paths::electron_exe_for(os, dir)
}

/// The OS (`std::env::consts::OS`'s word) of Electron's `slug` (`darwin-arm64`).
fn os_of(slug: &str) -> &str {
    match slug.split('-').next().unwrap_or("") {
        "darwin" => "macos",
        "win32" => "windows",
        o => o,
    }
}

/// The SHA-256 that `SHASUMS256.txt` lists for `file` (`<hex> *<file>`).
pub fn listed_sha(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hex, name) = l.split_once(char::is_whitespace)?;
        (name.trim().trim_start_matches('*') == file && hex.len() == 64).then(|| hex.to_lowercase())
    })
}

/// Slow links are fine (an hour for the body); a dead server is not.
pub(crate) fn config() -> ureq::config::Config {
    ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(30)))
        .timeout_recv_response(Some(std::time::Duration::from_secs(60)))
        .timeout_recv_body(Some(std::time::Duration::from_secs(3600)))
        .build()
}

/// aio's own runtime download: not the app's reach, so not held to its
/// `[capabilities]` (pinned, checked by SHA-256).
fn agent() -> ureq::Agent {
    config().into()
}

fn get_text(url: &str) -> io::Result<String> {
    let mut out = String::new();
    let mut body = agent().get(url).call().map_err(|e| io::Error::other(format!("{url}: {e}")))?.into_body();
    body.as_reader().take(1 << 20).read_to_string(&mut out)?;
    Ok(out)
}

/// Stream `url` into `to`, hashing as it goes (its SHA-256, hex); each
/// chunk's (got, total) to `on`, which says whether to go on (`Interrupted`
/// when not); every 10% to the log.
pub fn download(url: &str, to: &Path, on: &mut dyn FnMut(u64, Option<u64>) -> bool) -> io::Result<String> {
    let res = agent().get(url).call().map_err(|e| {
        let kind = match e {
            ureq::Error::StatusCode(404) => io::ErrorKind::NotFound,
            _ => io::ErrorKind::Other,
        };
        io::Error::new(kind, format!("{url}: {e}"))
    })?;
    let total: Option<u64> = res.headers().get("content-length").and_then(|v| v.to_str().ok()?.parse().ok());
    let total = total.filter(|t| *t > 0);
    let mut r = res.into_body().into_reader();
    let mut f = std::fs::File::create(to)?;
    let (mut hash, mut buf, mut got, mut said) = (Sha256::new(), vec![0; 1 << 16], 0u64, 0u64);
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        io::Write::write_all(&mut f, &buf[..n])?;
        got += n as u64;
        if !on(got, total) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, cancelled()));
        }
        if let Some(t) = total
            && got * 10 / t > said
        {
            said = got * 10 / t;
            eprintln!("[aio] {}: {}% of {} MB", url.rsplit('/').next().unwrap_or(url), said * 10, t >> 20);
        }
    }
    f.sync_all()?;
    Ok(aio::sha256::hex(&hash.finalize()))
}

fn cancelled() -> String {
    format!(
        "the start was cancelled while Electron {VERSION} was downloading: nothing was installed, \
         starting the app again downloads it"
    )
}

/// Where Electron's releases are: `AIORT_ELECTRON_MIRROR`, else GitHub.
fn mirror() -> String {
    std::env::var("AIORT_ELECTRON_MIRROR")
        .unwrap_or_else(|_| "https://github.com/electron/electron/releases/download/".into())
}

/// The runtime, fetched first if it isn't installed: on screen as `title`'s
/// (`firstrun`, in aio's colours), asking Retry or Quit when it fails.
/// `AIORT_ELECTRON_MIRROR` replaces
/// `https://github.com/electron/electron/releases/download/`.
pub fn ensure(title: &str) -> io::Result<PathBuf> {
    ensure_as(title, &crate::splash::colors(aio::colors::hue("aio"), None), None, None, None, None)
}

/// [`ensure`] for an app: its windows in its `colors` (`splash::colors`),
/// over its window's last place `at` (its middle, in Electron's points),
/// wearing its `icon` (a PNG file). With `back`, the failure question also
/// offers the version an update replaced, and `back` starts it. `splash`:
/// the app's splash image (its PNG and size on screen) drawn by the
/// download window, above its bar.
pub fn ensure_as(
    title: &str,
    colors: &[[String; 4]; 2],
    at: Option<(i32, i32)>,
    icon: Option<&Path>,
    splash: Option<(&[u8], (u32, u32))>,
    back: Option<&dyn Fn() -> io::Result<()>>,
) -> io::Result<PathBuf> {
    // Referenced, so no linker drops the marks from an app's binary.
    std::hint::black_box((PINNED, PINNED_SHAS));
    let dir = dir();
    if std::env::var_os("AIORT_RUNTIME").is_some() {
        return match exe(&dir).exists() {
            true => Ok(dir),
            false => {
                Err(io::Error::new(io::ErrorKind::NotFound, format!("AIORT_RUNTIME: no runtime in {}", dir.display())))
            }
        };
    }
    // A bundled runtime was fused when built, and is not written to: a
    // macOS one is sealed in the app's signature.
    if let Some(own) = bundled() {
        return Ok(own);
    }
    migrate();
    if usable(std::env::consts::OS, &dir) {
        stamp(&dir);
        return Ok(dir);
    }
    eprintln!("[aio] getting the Electron {VERSION} runtime (once per machine)…");
    let ui = crate::firstrun::here(std::env::consts::OS, &|k| std::env::var(k).ok(), &crate::firstrun::on_path);
    let look = crate::firstrun::Look { colors, at, icon, image: splash };
    let ask = |m: &str| {
        // The question takes over from the splash.
        crate::presplash::end(crate::presplash::Close::Failed);
        ui.as_ref().map_or(crate::firstrun::Answer::Quit, |b| b.ask(title, m, back.is_some(), &look))
    };
    let never = || Ok(());
    let sha = pinned_sha(VERSION);
    let fetched = || fetch(&dir, ui.as_ref(), title, VERSION, sha.as_deref(), &mirror(), &look, &|_| {});
    crate::firstrun::retrying(title, VERSION, ask, back.unwrap_or(&never), fetched)?;
    stamp(&dir);
    Ok(dir)
}

/// This start may download Electron: none was given, none is bundled, and
/// the shared one is not here yet.
pub(crate) fn may_download() -> bool {
    std::env::var_os("AIORT_RUNTIME").is_none() && bundled().is_none() && !usable(std::env::consts::OS, &dir())
}

/// In a runtime: when an app last started from it, an ISO time (aio's
/// `.aio-last-used`); `ax prune`, `am prune` and both auto-prunes read it.
pub const USED: &str = ".aio-last-used";

/// Say that an app starts from the runtime in `dir` now.
pub(crate) fn stamp_at(dir: &Path) -> io::Result<()> {
    std::fs::write(dir.join(USED), format!("{}\n", crate::prune::iso(std::time::SystemTime::now())))
}

/// [`stamp_at`], best effort: a read-only runtime is still a runtime.
fn stamp(dir: &Path) {
    _ = stamp_at(dir);
}

/// The URL Electron publishes `version`'s zip for `slug` at.
fn zip_url(mirror: &str, version: &str, slug: &str) -> String {
    format!("{}/v{version}/electron-v{version}-{slug}.zip", mirror.trim_end_matches('/'))
}

/// Install `version` for `slug` into the store entry `dir` as aio does
/// (`ensureElectronRuntime` `into`), unless another process just did: under
/// its lock, staged beside it, the zip checked against `sha`, unpacked,
/// fused (and signed again, ad hoc, on a Mac), stamped complete, renamed in.
/// `on` gets each chunk's (got, total), and (0, None) while another
/// download is waited for; false cancels. The error says if it was the
/// download that failed.
fn install_into(
    dir: &Path,
    version: &str,
    slug: &str,
    sha: &str,
    mirror: &str,
    on: &mut dyn FnMut(u64, Option<u64>) -> bool,
) -> Result<(), (io::Error, bool)> {
    let os = os_of(slug);
    if usable(os, dir) {
        return Ok(());
    }
    let parent = dir.parent().ok_or_else(|| (io::Error::other("runtime dir has no parent"), false))?;
    // A store that can't be written says so now, not after the lock's wait.
    let probe = parent.join(format!(".aio-write-probe.{}", crate::pidlock::stage_tag()));
    if let Err(e) = std::fs::create_dir_all(parent).and_then(|()| std::fs::write(&probe, "")) {
        let why = format!(
            "cannot write to {} ({e}), so the Electron runtime cannot be installed there. Make it writable, or point \
             $AIO_SHARED_ELECTRON_DIR at a folder that is.",
            parent.display()
        );
        return Err((io::Error::new(e.kind(), why), false));
    }
    _ = std::fs::remove_file(&probe);
    let lock = parent.join(format!("{}.lock", dir.file_name().unwrap_or_default().to_string_lossy()));
    let (on, stopped) = (std::cell::RefCell::new(on), std::cell::Cell::new(false));
    let mut waiting = || {
        stopped.set(!(*on.borrow_mut())(0, None));
        !stopped.get()
    };
    crate::pidlock::with_lock(&lock, LOCK_WAIT, &mut waiting, |held| {
        if usable(os, dir) {
            return Ok(());
        }
        if !held {
            return Err((
                match stopped.get() {
                    true => io::Error::new(io::ErrorKind::Interrupted, cancelled()),
                    false => io::Error::other(format!(
                        "another process has been downloading the Electron runtime {version} ({slug}) into {} for over \
                         {} minutes and is still alive. Wait for it. If no such process exists, delete {}.",
                        dir.display(),
                        LOCK_WAIT.as_secs() / 60,
                        lock.display()
                    )),
                },
                true,
            ));
        }
        let stage = parent.join(format!(
            "{}.incoming.{}",
            dir.file_name().unwrap_or_default().to_string_lossy(),
            crate::pidlock::stage_tag()
        ));
        crate::pidlock::remove_dead_stages(dir);
        _ = std::fs::remove_dir_all(&stage);
        std::fs::create_dir_all(&stage).map_err(|e| (e, false))?;
        let fresh = crate::pidlock::keep_fresh(stage.clone(), || true);
        let r = staged(&stage, dir, os, version, slug, sha, mirror, &mut **on.borrow_mut());
        drop(fresh);
        _ = std::fs::remove_dir_all(&stage);
        r
    })
}

/// [`install_into`]'s work in its `stage`, holding the lock.
#[allow(clippy::too_many_arguments)]
fn staged(
    stage: &Path,
    dir: &Path,
    os: &str,
    version: &str,
    slug: &str,
    sha: &str,
    mirror: &str,
    on: &mut dyn FnMut(u64, Option<u64>) -> bool,
) -> Result<(), (io::Error, bool)> {
    let url = zip_url(mirror, version, slug);
    let file = format!("electron-v{version}-{slug}.zip");
    let zip = stage.join("electron.zip");
    let got = download(&url, &zip, on).map_err(|e| (e, true))?;
    if got != sha.to_lowercase() {
        let why = format!(
            "integrity check FAILED for {file}: this app's build pinned {sha}, the download hashes to {got}. It is not \
             the Electron the app was built with: corrupted, or replaced on the way by a mirror, a proxy or the \
             network. It is NOT being unpacked."
        );
        return Err((io::Error::new(io::ErrorKind::InvalidData, why), true));
    }
    let unpacked = std::fs::File::open(&zip)
        .and_then(|z| zip::ZipArchive::new(z).map_err(io::Error::other))
        .and_then(|mut a| a.extract(stage).map_err(io::Error::other));
    _ = std::fs::remove_file(&zip);
    unpacked.map_err(|e| (e, false))?;
    if !exe_for(os, stage).is_file() {
        let why = format!("{file} unpacked without its executable: the archive is not an Electron runtime for {slug}.");
        return Err((io::Error::new(io::ErrorKind::InvalidData, why), false));
    }
    let finish = || -> io::Result<()> {
        fuses_off(os, stage)?;
        if os == "macos" && std::env::consts::OS == "macos" {
            codesign(&stage.join("Electron.app"), true)?;
        }
        std::fs::write(stage.join(COMPLETE), format!("{url}\nsha256:{got}\n"))?;
        // One that took over a lock it thought dead may have finished first:
        // a runtime whose Electron may be starting is not replaced.
        if usable(os, dir) {
            return Ok(());
        }
        _ = std::fs::remove_dir_all(dir);
        aio::paths::rename_over(stage, dir)
    };
    finish().map_err(|e| (e, false))
}

/// One go at fetching the runtime into `dir`, in a window when there is
/// `ui`, checked against `sha` (none is refused, as aio refuses it). The
/// error says if it was the download that failed. `step` hears the
/// download's percent.
#[allow(clippy::too_many_arguments)]
fn fetch(
    dir: &Path,
    ui: Option<&crate::firstrun::Backend>,
    title: &str,
    version: &str,
    sha: Option<&str>,
    mirror: &str,
    look: &crate::firstrun::Look,
    step: &dyn Fn(u8),
) -> Result<(), (io::Error, bool)> {
    use crate::firstrun::{Shown, download_detail};
    let open = || {
        ui.and_then(|b| {
            b.progress(title, &crate::firstrun::heading(crate::firstrun::words(), title, version), look)
                .inspect_err(|e| eprintln!("[aio] runtime: no progress window ({e}): the log carries the progress"))
                .ok()
        })
    };
    // The app's splash (`presplash`) is the download's screen while it is
    // up; once it is gone, the download window takes over.
    let mut native = crate::presplash::current().filter(|s| s.download());
    let _done = native.clone().map(Downloaded);
    let mut win = if native.is_some() { None } else { open() };
    let mut show = |pct: Option<u64>, detail: &str| {
        if let Some(s) = &native {
            match s.progress(pct, detail) {
                Shown::On => return true,
                Shown::Cancelled => return false,
                Shown::Lost(_) => {
                    native = None;
                    win = open();
                }
            }
        }
        match win.as_mut().map(|w| w.show(pct, detail)) {
            Some(Shown::Cancelled) => false,
            Some(Shown::Lost(why)) => {
                eprintln!(
                    "[aio] runtime: the progress window could not be shown ({why}): the log carries the progress"
                );
                win = None;
                true
            }
            _ => true,
        }
    };
    if !show(None, &format!("Looking for Electron {version}…")) {
        return Err((io::Error::new(io::ErrorKind::Interrupted, cancelled()), true));
    }
    let slug = slug();
    let Some(sha) = sha else {
        let why = format!(
            "this build carries no checksum for Electron {version} on {slug}, so a download could not be told from a \
             forgery: rebuild the app (its build pins one), or use [build] runtime = \"bundled\""
        );
        return Err((io::Error::new(io::ErrorKind::InvalidData, why), false));
    };
    install_into(dir, version, &slug, sha, mirror, &mut |got, total| {
        if let Some(t) = total.filter(|&t| t > 0) {
            step((got.min(t) * 100 / t) as u8);
        }
        match total {
            _ if got == 0 && total.is_none() => show(None, &format!("Looking for Electron {version}…")),
            Some(t) if got >= t => show(None, &format!("Verifying and unpacking Electron {version}…")),
            _ => show(
                total.map(|t| got * 100 / t),
                &download_detail(crate::firstrun::words(), title, version, got, total),
            ),
        }
    })
}

/// The download is over, however it ended: the splash shows the start's
/// text again, and its cap counts from now.
struct Downloaded(std::sync::Arc<crate::presplash::Session>);

impl Drop for Downloaded {
    fn drop(&mut self) {
        self.0.downloaded();
    }
}

/// Why Chromium's sandbox can't start on this Linux, or `None`.
/// Chromium sandboxes through unprivileged user namespaces when it may, and
/// only then needs nothing else; restricted (Ubuntu 23.10+'s AppArmor sysctl,
/// Debian's switch, a zero namespace cap, a container's seccomp), it needs
/// its `chrome-sandbox` helper setuid root, which an unprivileged download
/// can't be, and refuses to start at all. `read`: a sysctl's text; `probe`:
/// can this process make a user namespace; `helper`: its (uid, mode).
pub fn sandbox_missing(
    read: impl Fn(&str) -> String,
    probe: impl FnOnce() -> bool,
    helper: Option<(u32, u32)>,
) -> Option<String> {
    let sysctl = |p: &str| read(p).trim().to_string();
    let closed = if sysctl("/proc/sys/kernel/apparmor_restrict_unprivileged_userns") == "1" {
        Some("AppArmor restricts unprivileged user namespaces")
    } else if sysctl("/proc/sys/kernel/unprivileged_userns_clone") == "0" {
        Some("unprivileged user namespaces are switched off")
    } else if sysctl("/proc/sys/user/max_user_namespaces") == "0" {
        Some("user namespaces are capped at 0")
    } else if !probe() {
        Some("making a user namespace is refused (a container?)")
    } else {
        None
    }?;
    match helper {
        // No helper: nothing to misconfigure, Chromium picks another way.
        None => None,
        Some((0, mode)) if mode & 0o4000 != 0 => None,
        Some(_) => Some(format!("{closed}, and chrome-sandbox is not setuid root")),
    }
}

/// The flag Electron needs on this machine, said every time it is used.
pub fn sandbox_args(dir: &Path) -> Vec<String> {
    sandbox_for(dir, false).unwrap_or_default()
}

/// [`sandbox_args`], or with `require` (`Window { require_sandbox }`, or
/// `AIORT_SANDBOX=1`) the reason there will be no window: a host that
/// can't sandbox stops the app rather than run its page unsandboxed.
pub fn sandbox_for(#[cfg_attr(not(unix), allow(unused))] dir: &Path, require: bool) -> io::Result<Vec<String>> {
    if std::env::consts::OS != "linux" {
        return Ok(vec![]);
    }
    let probe = || {
        let mut cmd = std::process::Command::new("unshare");
        cmd.args(["-U", "--", "true"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        // No `unshare` here says nothing about the kernel: assume allowed.
        cmd.status().map_or(true, |s| s.success())
    };
    #[cfg(unix)]
    let helper = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(dir.join("chrome-sandbox")).ok().map(|m| (m.uid(), m.mode()))
    };
    #[cfg(not(unix))]
    let helper = None;
    let missing = sandbox_missing(|p| std::fs::read_to_string(p).unwrap_or_default(), probe, helper);
    let by_env = std::env::var_os("AIORT_SANDBOX").is_some_and(|v| v == "1");
    no_sandbox(missing, require, by_env).map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied, e))
}

/// The runtime's switches for a host whose sandbox is `missing` (why it
/// can't start), or why the app stops instead (pure).
pub(crate) fn no_sandbox(missing: Option<String>, require: bool, by_env: bool) -> Result<Vec<String>, String> {
    let Some(why) = missing else { return Ok(vec![]) };
    let who = match (require, by_env) {
        (true, _) => Some("this app requires it (Window { require_sandbox: true })"),
        (false, true) => Some("AIORT_SANDBOX=1 requires it"),
        _ => None,
    };
    match who {
        Some(who) => Err(format!(
            "Chromium's sandbox can't start here ({why}), and {who}: the window does not open unsandboxed. Make \
             chrome-sandbox root-owned 4755, or allow unprivileged user namespaces"
        )),
        None => {
            eprintln!(
                "[aio] runtime: {why}: starting it with --no-sandbox (the page is this app's own UI; set \
                 AIORT_SANDBOX=1 or Window {{ require_sandbox: true }} to refuse instead, or make chrome-sandbox \
                 root-owned 4755)"
            );
            Ok(vec!["--no-sandbox".into()])
        }
    }
}

/// In an offline build's runtime, and in a pre-0.1.0 build's store: the fuses
/// are off. (A store entry's `.aio-complete` says that and more.)
const FUSED: &str = ".aio-fused";

/// The binary that holds the fuse wire of a runtime for `os`: the
/// framework on macOS.
fn fused_binary(os: &str, dir: &Path) -> PathBuf {
    match os {
        "macos" => {
            dir.join("Electron.app/Contents/Frameworks/Electron Framework.framework/Versions/A/Electron Framework")
        }
        _ => exe_for(os, dir),
    }
}

/// Turn the fuses of the runtime for `os` in `dir` off; whether one was on.
fn fuses_off(os: &str, dir: &Path) -> io::Result<bool> {
    let bin = fused_binary(os, dir);
    let mut b = std::fs::read(&bin)?;
    let bad = |e: String| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", bin.display()));
    if crate::fuses::are_off(&b).map_err(bad)? {
        return Ok(false);
    }
    crate::fuses::turn_off(&mut b).map_err(bad)?;
    let tmp = bin.with_extension("fusing");
    std::fs::write(&tmp, &b)?;
    std::fs::set_permissions(&tmp, std::fs::metadata(&bin)?.permissions())?;
    std::fs::rename(&tmp, &bin)?;
    Ok(true)
}

/// `codesign --force [--deep] --sign - <path>`: an ad hoc signature.
fn codesign(path: &Path, deep: bool) -> io::Result<()> {
    let out = std::process::Command::new("/usr/bin/codesign")
        .arg("--force")
        .args(deep.then_some("--deep"))
        .args(["--sign", "-"])
        .arg(path)
        .output()?;
    if out.status.success() {
        return Ok(());
    }
    let why = String::from_utf8_lossy(&out.stderr);
    Err(io::Error::other(format!("codesign {}: {}", path.display(), why.trim().lines().last().unwrap_or(""))))
}

/// Set `key` in an XML plist (Electron's own, fixed format) to the string
/// `value`, adding it before the last `</dict>` when absent (pure).
pub fn set_plist(plist: &str, key: &str, value: &str) -> String {
    let esc = value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let tag = format!("<key>{key}</key>");
    if let Some(at) = plist.find(&tag) {
        let rest = &plist[at + tag.len()..];
        if let (Some(open), Some(close)) = (rest.find("<string>"), rest.find("</string>"))
            && rest[..open].trim().is_empty()
        {
            let from = at + tag.len() + open + "<string>".len();
            return format!("{}{esc}{}", &plist[..from], &plist[at + tag.len() + close..]);
        }
    }
    match plist.rfind("</dict>") {
        Some(at) => format!("{}\t<key>{key}</key>\n\t<string>{esc}</string>\n{}", &plist[..at], &plist[at..]),
        None => plist.to_string(),
    }
}

/// What a macOS window bundle shows: the app's name, id, version, icon.
pub struct Identity<'a> {
    pub id: &'a str,
    /// `CFBundleIdentifier`, the `.app`'s (`Config::mac_bundle_id`).
    pub bundle_id: &'a str,
    pub name: &'a str,
    pub version: &'a str,
    /// The app's PNG icon, if any.
    pub icon: Option<&'a Path>,
}

/// macOS: the app's own window bundle, so the Dock shows the app, not
/// "Electron". A clone of the shared `Electron.app` (APFS clones cost
/// no disk) in `electron/windows/<id>`, stamped with the app's identity and
/// signed ad hoc; made again when the runtime or the identity changes.
/// Returns the executable to start the window from.
pub fn window(rt: &Path, who: &Identity) -> io::Result<PathBuf> {
    let home = window_home(who.id);
    let bundle = home.join("Electron.app");
    let exe = bundle.join("Contents/MacOS/Electron");
    let icon_sha = match who.icon {
        Some(p) => aio::sha256::hex(&Sha256::digest(std::fs::read(p)?)),
        None => String::new(),
    };
    let stamp = format!(
        "{}\n{}\n{}\n{}\n{}\n{icon_sha}\n",
        rt.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        who.id,
        who.bundle_id,
        who.name,
        who.version
    );
    let stamp_file = home.join(".aio-window");
    if exe.exists() && std::fs::read_to_string(&stamp_file).is_ok_and(|s| s == stamp) {
        return Ok(exe);
    }
    std::fs::create_dir_all(&home)?;
    let stage = home.join(format!(".stage-{}", std::process::id()));
    _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)?;
    let made = (|| {
        let staged = stage.join("Electron.app");
        let src = rt.join("Electron.app");
        let cp = |clone: bool| {
            std::process::Command::new("/bin/cp")
                .args(clone.then_some("-c"))
                .args(["-R", "-P"])
                .arg(&src)
                .arg(&staged)
                .status()
                .is_ok_and(|s| s.success())
        };
        // A volume that can't clone gets a plain copy.
        if !cp(true) {
            _ = std::fs::remove_dir_all(&staged);
            if !cp(false) {
                return Err(io::Error::other(format!("could not copy {}", src.display())));
            }
        }
        let resources = staged.join("Contents/Resources");
        let icon_file = match who.icon {
            Some(png) => {
                // Electron's own icon goes; the app's takes its place.
                _ = std::fs::remove_file(resources.join("electron.icns"));
                let ok = std::process::Command::new("/usr/bin/sips")
                    .args(["-s", "format", "icns"])
                    .arg(png)
                    .arg("--out")
                    .arg(resources.join("AppIcon.icns"))
                    .output()
                    .is_ok_and(|o| o.status.success());
                if ok { "AppIcon" } else { "electron" }
            }
            None => "electron",
        };
        let contents = staged.join("Contents");
        let helpers = ["", " (GPU)", " (Plugin)", " (Renderer)"]
            .map(|s| contents.join(format!("Frameworks/Electron Helper{s}.app/Contents/Info.plist")));
        for plist in std::iter::once(contents.join("Info.plist")).chain(helpers) {
            let Ok(mut text) = std::fs::read_to_string(&plist) else { continue };
            for (k, v) in [
                ("CFBundleName", who.name),
                ("CFBundleDisplayName", who.name),
                ("CFBundleIdentifier", who.bundle_id),
                ("CFBundleIconFile", icon_file),
                ("CFBundleShortVersionString", who.version),
                ("CFBundleVersion", who.version),
            ] {
                text = set_plist(&text, k, v);
            }
            std::fs::write(&plist, text)?;
        }
        // The stamped plists are signed again, and only those: the framework
        // keeps the runtime's signature, and stays a clone (no disk).
        for e in std::fs::read_dir(contents.join("Frameworks"))?.flatten() {
            if e.file_name().to_string_lossy().ends_with(".app") {
                codesign(&e.path(), false)?;
            }
        }
        codesign(&staged, false)?;
        _ = std::fs::remove_dir_all(&bundle);
        std::fs::rename(&staged, &bundle)?;
        std::fs::write(&stamp_file, &stamp)
    })();
    _ = std::fs::remove_dir_all(&stage);
    made.map(|()| exe)
}

/// App `id`'s macOS window bundle's folder: `windows/<id>` in the store,
/// what its record names (`window`) so no prune takes it.
pub fn window_home(id: &str) -> PathBuf {
    root().join("windows").join(id)
}

/// Move what a pre-0.1.0 build kept in its own store into aio's, once, best
/// effort (each failure said; the next start tries again).
pub fn migrate() {
    let old = aio::paths::old_electron_store();
    if old.is_dir() {
        migrate_from(&old, &root(), &crate::prune::in_use);
    }
}

/// [`migrate`] from `old` into the store `root` (`busy`: a program runs
/// from that folder). Each runtime under its lock in `root`: deleted when
/// the store has it complete, else moved in one rename into a stage,
/// fused, stamped complete and renamed into place. Records become aio's;
/// windows (made again at the next start) and stamps go; `old` goes once
/// empty. One a program runs from stays until it is not.
pub fn migrate_from(old: &Path, root: &Path, busy: &dyn Fn(&Path) -> bool) {
    let say = |what: &str, e: &dyn std::fmt::Display| {
        eprintln!(
            "[aio] runtime: could not move {what} from {} into {} ({e}); the next start tries again",
            old.display(),
            root.display()
        );
    };
    for e in std::fs::read_dir(old).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        let Some((version, slug)) =
            name.split_once('-').filter(|(v, s)| aio::update::parse_version(v).is_some() && s.contains('-'))
        else {
            // A download 0.1.0 left: gone once an hour old.
            let stale = || e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
            if name.starts_with('.')
                && name.contains(".partial-")
                && stale().is_some_and(|q| q > crate::pidlock::STAGE_STALE)
            {
                _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
            }
            continue;
        };
        if !path.is_dir() || busy(&path) {
            continue;
        }
        let (os, dir) = (os_of(slug), root.join(&name));
        if let Err(e) = std::fs::create_dir_all(root) {
            return say(&name, &e);
        }
        let lock = root.join(format!("{name}.lock"));
        let moved = crate::pidlock::with_lock(&lock, Duration::from_secs(1), &mut || true, |held| -> io::Result<()> {
            if !held {
                return Ok(()); // being installed now: the next start
            }
            let aside = |p: &Path| p.with_file_name(format!("{}.incoming.{}", name, crate::pidlock::stage_tag()));
            if usable(os, &dir) || !exe_for(os, &path).is_file() {
                let gone = aside(&path);
                return std::fs::rename(&path, &gone).and_then(|()| std::fs::remove_dir_all(&gone));
            }
            let stage = aside(&dir);
            std::fs::rename(&path, &stage)?;
            let finished = (|| {
                if fuses_off(os, &stage)? && os == "macos" && std::env::consts::OS == "macos" {
                    codesign(&stage.join("Electron.app"), true)?;
                }
                // Its last start, as aio stamps it.
                let used =
                    std::fs::read_to_string(stage.join(".aio-used")).ok().and_then(|ms| ms.trim().parse::<u64>().ok());
                if let Some(ms) = used {
                    let at = std::time::UNIX_EPOCH + Duration::from_millis(ms);
                    std::fs::write(stage.join(USED), format!("{}\n", crate::prune::iso(at)))?;
                }
                for f in [".aio-used", FUSED] {
                    _ = std::fs::remove_file(stage.join(f));
                }
                let sha = (version == VERSION).then(|| sha_for(electron_sha256!(), slug)).flatten();
                let url = zip_url("https://github.com/electron/electron/releases/download/", version, slug);
                std::fs::write(
                    stage.join(COMPLETE),
                    format!("{url}\nsha256:{}\n", sha.as_deref().unwrap_or("unknown")),
                )?;
                _ = std::fs::remove_dir_all(&dir);
                aio::paths::rename_over(&stage, &dir)
            })();
            if finished.is_err() && std::fs::rename(&stage, &path).is_err() {
                _ = std::fs::remove_dir_all(&stage);
            }
            finished
        });
        if let Err(e) = moved {
            say(&name, &e);
        }
    }
    // Its records, in aio's shape: what they protect stays protected.
    for e in std::fs::read_dir(old.join("apps")).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let exe = std::fs::read_to_string(e.path()).unwrap_or_default();
        let exe = exe.trim_end_matches(['\n', '\r']);
        let rt = name.rsplit_once('@').map(|(_, rt)| rt).and_then(|rt| rt.split_once('-'));
        let r = match rt {
            Some((version, slug)) if !exe.is_empty() && Path::new(exe).exists() => {
                let at = e.metadata().and_then(|m| m.modified()).unwrap_or_else(|_| std::time::SystemTime::now());
                let u = crate::prune::Use {
                    exe: exe.into(),
                    version: version.into(),
                    slug: slug.into(),
                    ..Default::default()
                };
                crate::prune::record_at(root, &u, at)
            }
            _ => Ok(()), // its app is gone, or it never was a record
        };
        match r.and_then(|()| std::fs::remove_file(e.path())) {
            Ok(()) => {}
            Err(err) => say(&format!("apps/{name}"), &err),
        }
    }
    for e in std::fs::read_dir(old.join("windows")).into_iter().flatten().flatten() {
        if !busy(&e.path()) {
            _ = std::fs::remove_dir_all(e.path());
        }
    }
    _ = std::fs::remove_file(old.join(".aio-auto-prune"));
    for d in [old.join("apps"), old.join("windows"), old.to_path_buf()] {
        _ = std::fs::remove_dir(d);
    }
}

/// Unpack `zip` into `dir` through a staging dir and one rename.
pub fn install(zip: impl Read + io::Seek, dir: &Path) -> io::Result<()> {
    install_for(std::env::consts::OS, zip, dir)
}

/// [`install`] of a runtime for `os`.
fn install_for(os: &str, zip: impl Read + io::Seek, dir: &Path) -> io::Result<()> {
    let parent = dir.parent().ok_or_else(|| io::Error::other("runtime dir has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let stage = parent.join(format!(
        ".{}.partial-{}",
        dir.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    _ = std::fs::remove_dir_all(&stage);
    let mut archive = zip::ZipArchive::new(zip).map_err(io::Error::other)?;
    let r = archive.extract(&stage).map_err(io::Error::other).and_then(|_| {
        if !exe_for(os, &stage).exists() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "the archive has no runtime executable"));
        }
        std::fs::rename(&stage, dir)
    });
    if r.is_err() {
        _ = std::fs::remove_dir_all(&stage);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_stamps_the_runtime_it_starts_from() {
        let d = tempfile::tempdir().unwrap();
        stamp(d.path());
        // aio's `.aio-last-used`: an ISO time and a newline.
        let text = std::fs::read_to_string(d.path().join(USED)).unwrap();
        let at = crate::prune::parse_iso(&text).unwrap();
        assert!(text.ends_with("Z\n") && at.elapsed().unwrap_or_default() < Duration::from_secs(60));
        // A runtime it can't write to still starts.
        stamp(&d.path().join("missing"));
    }

    #[test]
    fn a_plist_value_is_set_or_added() {
        let p = "<dict>\n\t<key>CFBundleName</key>\n\t<string>Electron</string>\n</dict>";
        let p = set_plist(p, "CFBundleName", "Notes & Co");
        assert!(p.contains("<key>CFBundleName</key>\n\t<string>Notes &amp; Co</string>"), "{p}");
        let p = set_plist(&p, "CFBundleDisplayName", "Notes");
        assert!(p.ends_with("<key>CFBundleDisplayName</key>\n\t<string>Notes</string>\n</dict>"), "{p}");
        assert_eq!(p.matches("CFBundleName").count(), 1);
    }
    use std::io::Write;

    #[test]
    fn the_sandbox_is_dropped_only_when_it_cant_start_and_that_is_said() {
        let host = |restrict: &'static str| {
            move |p: &str| {
                if p.ends_with("apparmor_restrict_unprivileged_userns") { restrict.into() } else { String::new() }
            }
        };
        // Namespaces open: Chromium never needs the helper.
        assert_eq!(sandbox_missing(host("0"), || true, Some((1000, 0o755))), None);
        // Closed, helper unprivileged (a download): no window without --no-sandbox.
        let why = sandbox_missing(host("1\n"), || true, Some((1000, 0o755))).unwrap();
        assert!(why.contains("AppArmor") && why.contains("not setuid root"), "{why}");
        // Closed, but the helper is set up properly, or absent.
        assert_eq!(sandbox_missing(host("1"), || true, Some((0, 0o104755))), None);
        assert_eq!(sandbox_missing(host("1"), || true, None), None);
        // A container: every sysctl says yes, the syscall says no.
        assert!(sandbox_missing(host(""), || false, Some((1000, 0o755))).unwrap().contains("container"));
        let off = |p: &str| if p.ends_with("unprivileged_userns_clone") { "0".into() } else { String::new() };
        assert!(sandbox_missing(off, || true, Some((1000, 0o755))).is_some());
    }

    /// aio's requireSandbox: a host that can't sandbox stops the app, with
    /// the reason and the way out; elsewhere `--no-sandbox`, said.
    #[test]
    fn a_required_sandbox_stops_the_app_rather_than_run_without_one() {
        let why = || Some("AppArmor restricts unprivileged user namespaces".to_string());
        assert_eq!(no_sandbox(None, true, true), Ok(vec![]), "a host that sandboxes needs nothing");
        assert_eq!(no_sandbox(why(), false, false), Ok(vec!["--no-sandbox".to_string()]));
        let e = no_sandbox(why(), true, false).unwrap_err();
        assert!(e.contains("AppArmor") && e.contains("require_sandbox: true") && e.contains("4755"), "{e}");
        assert!(no_sandbox(why(), false, true).unwrap_err().contains("AIORT_SANDBOX=1"));
    }

    #[test]
    fn the_download_reports_each_chunk_and_stops_on_a_cancel() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/x.zip", l.local_addr().unwrap());
        let body = vec![7u8; 300_000];
        let served = body.clone();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut s, _) = l.accept().unwrap();
                _ = s.read(&mut [0; 4096]);
                _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", served.len()).as_bytes());
                _ = s.write_all(&served);
            }
        });
        let d = tempfile::tempdir().unwrap();
        let mut seen = vec![];
        let sha = download(&url, &d.path().join("a"), &mut |got, total| {
            seen.push((got, total));
            true
        })
        .unwrap();
        assert_eq!(sha, aio::sha256::hex(&Sha256::digest(&body)));
        assert_eq!(seen.last(), Some(&(300_000, Some(300_000))));
        let mut asked = 0;
        let e = download(&url, &d.path().join("b"), &mut |_, _| {
            asked += 1;
            false
        })
        .unwrap_err();
        assert_eq!((e.kind(), asked), (io::ErrorKind::Interrupted, 1));
        assert!(e.to_string().contains("nothing was installed"), "{e}");
    }

    #[test]
    fn a_binary_says_which_runtime_it_needs() {
        assert_eq!(pinned_in(PINNED).as_deref(), Some(VERSION));
        // This test binary links the mark, as every app does.
        let me = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        assert!(me.windows(PINNED.len()).any(|w| w == PINNED));
        assert_eq!(pinned_in(b"..\0aio-electron-runtime=45.0.1\0.."), Some("45.0.1".into()));
        assert_eq!(pinned_in(b"an old build"), None);
        assert_eq!(pinned_in(b"\0aio-electron-runtime=\x01\0\0aio-electron-runtime=45.0.1\0"), Some("45.0.1".into()));
        assert_eq!(pinned_in(b"\0aio-electron-runtime=../x\0"), None, "only a version");
        assert_eq!(pinned_in(b"\0aio-electron-runtime=45.0.1"), None, "cut short");
    }

    /// What is in `root`, sorted.
    #[cfg(not(target_os = "macos"))]
    fn left(root: &Path) -> Vec<String> {
        let mut v: Vec<String> =
            std::fs::read_dir(root).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        v.sort();
        v
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_runtime_is_installed_as_aio_installs_it_checked_against_the_pin() {
        let name = exe(Path::new("")).to_string_lossy().into_owned();
        let (mirror, asked) = mirror_of("45.0.1", &slug(), &[(&name, &fake_electron())]);
        let zip_sha = |m: &str| {
            let got = get_text(&format!("{m}v45.0.1/SHASUMS256.txt")).unwrap();
            listed_sha(&got, &format!("electron-v45.0.1-{}.zip", slug())).unwrap()
        };
        let sha = zip_sha(&mirror);
        let t = tempfile::tempdir().unwrap();
        // Another pin than the zip's: refused, nothing installed, nothing left.
        let e = prefetch_in(t.path(), "45.0.1", Some(&"0".repeat(64)), &mirror, &|_| {}).unwrap_err();
        assert!(
            e.to_string().contains("integrity check FAILED") && e.to_string().contains("NOT being unpacked"),
            "{e}"
        );
        assert_eq!(left(t.path()), Vec::<String>::new());
        // No pin at all: refused before anything is fetched.
        let before = asked.load(std::sync::atomic::Ordering::SeqCst);
        let e = prefetch_in(t.path(), "45.0.1", None, &mirror, &|_| {}).unwrap_err();
        assert!(e.to_string().contains("carries no checksum"), "{e}");
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), before);
        let seen = std::sync::Mutex::new(vec![]);
        let dir = prefetch_in(t.path(), "45.0.1", Some(&sha), &mirror, &|p| seen.lock().unwrap().push(p)).unwrap();
        let seen = seen.into_inner().unwrap();
        assert!(seen.last() == Some(&100) && seen.is_sorted(), "the download's progress is heard: {seen:?}");
        let entry = format!("45.0.1-{}", slug());
        assert!(dir.ends_with(&entry) && usable(std::env::consts::OS, &dir));
        // aio's stamps, word for word; the fuses off; no lock, stage or probe left.
        let url = format!("{mirror}v45.0.1/electron-v45.0.1-{}.zip", slug());
        assert_eq!(std::fs::read_to_string(dir.join(COMPLETE)).unwrap(), format!("{url}\nsha256:{sha}\n"));
        assert!(crate::prune::parse_iso(&std::fs::read_to_string(dir.join(USED)).unwrap()).is_some());
        assert!(crate::fuses::are_off(&std::fs::read(exe(&dir)).unwrap()).unwrap());
        assert_eq!(left(t.path()), std::slice::from_ref(&entry));
        // There already: nothing is asked.
        assert_eq!(prefetch_in(t.path(), "45.0.1", Some(&sha), "http://127.0.0.1:1/", &|_| {}).unwrap(), dir);
        // A tree with no `.aio-complete` is not a runtime (aio's half-done one).
        std::fs::remove_file(dir.join(COMPLETE)).unwrap();
        assert!(!usable(std::env::consts::OS, &dir));
        prefetch_in(t.path(), "45.0.1", Some(&sha), &mirror, &|_| {}).unwrap();
        assert!(usable(std::env::consts::OS, &dir));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_download_another_process_holds_is_waited_for_and_a_cancel_ends_the_wait() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join(format!("45.0.1-{}", slug()));
        let lock = t.path().join(format!("45.0.1-{}.lock", slug()));
        // Held by a live process (the test harness's parent).
        #[cfg(unix)]
        std::fs::write(&lock, std::os::unix::process::parent_id().to_string()).unwrap();
        #[cfg(not(unix))]
        assert!(crate::pidlock::try_lock(&lock).0);
        let mut asked = 0;
        let e = install_into(&dir, "45.0.1", &slug(), &"0".repeat(64), "http://127.0.0.1:1/", &mut |_, _| {
            asked += 1;
            asked < 3
        })
        .unwrap_err();
        assert_eq!((e.0.kind(), asked), (io::ErrorKind::Interrupted, 3));
        assert!(!dir.exists());
    }

    #[test]
    fn pins_are_the_releases_and_travel_in_the_binary() {
        // Electron's v44.9.3 SHASUMS256.txt, the list aio's build pins from.
        assert_eq!(
            sha_for(electron_sha256!(), "linux-x64").as_deref(),
            Some("6e7fd2d450d442507b846ace4ae6e2bd17e1b1653e86a033b5231abfc23e6df4")
        );
        assert_eq!(
            sha_for(electron_sha256!(), "win32-x64").as_deref(),
            Some("41a1ffa0541cb0f2c9b9fe920dd2cef1793ad0ea2ada1a9582958880f4d1b23c")
        );
        assert_eq!(sha_for(electron_sha256!(), "linux-ia32"), None);
        assert_eq!(pinned_sha("1.0.0"), None, "only the version this build pins");
        if matches!(
            slug().as_str(),
            "linux-x64" | "linux-arm64" | "win32-x64" | "win32-arm64" | "darwin-x64" | "darwin-arm64"
        ) {
            assert!(pinned_sha(VERSION).is_some());
            // This test binary carries its list, as every app does.
            let me = std::fs::read(std::env::current_exe().unwrap()).unwrap();
            assert_eq!(sha_in(&me, VERSION), pinned_sha(VERSION));
        }
        let other = format!("..\0aio-electron-sha256={}={} x=y\0..", slug(), "ab".repeat(32));
        assert_eq!(sha_in(other.as_bytes(), "45.0.1"), Some("ab".repeat(32)));
        assert_eq!(sha_in(b"an old build", "45.0.1"), None);
        assert_eq!(sha_in(b"an old build", VERSION), pinned_sha(VERSION));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn aio_rs_0_1_0s_store_moves_into_aios_once() {
        let t = tempfile::tempdir().unwrap();
        let (old, root) = (t.path().join("aio-rt/electron"), t.path().join("aio/electron"));
        let (moved, there) = (format!("45.0.1-{}", slug()), format!("45.0.2-{}", slug()));
        let bin = |d: &Path| {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(exe(d), fake_electron()).unwrap();
        };
        bin(&old.join(&moved));
        std::fs::write(old.join(&moved).join(".aio-used"), "1730000000123").unwrap();
        std::fs::write(old.join(&moved).join(FUSED), "").unwrap();
        bin(&old.join(&there));
        // The store already has 45.0.2, complete: the old copy just goes.
        bin(&root.join(&there));
        std::fs::write(root.join(&there).join(COMPLETE), "aio's\n").unwrap();
        let live = t.path().join("notes");
        std::fs::write(&live, "").unwrap();
        std::fs::create_dir_all(old.join("apps")).unwrap();
        std::fs::write(old.join(format!("apps/notes@{moved}")), live.to_string_lossy().as_bytes()).unwrap();
        std::fs::write(old.join(format!("apps/gone@{moved}")), t.path().join("gone").to_string_lossy().as_bytes())
            .unwrap();
        std::fs::create_dir_all(old.join("windows/notes")).unwrap();
        std::fs::write(old.join(".aio-auto-prune"), "").unwrap();
        // A folder a program runs from stays where it is, for now.
        let busy_one = old.join(&moved);
        migrate_from(&old, &root, &|p| p == busy_one);
        assert!(old.join(&moved).is_dir() && !root.join(&moved).exists());
        migrate_from(&old, &root, &|_| false);
        assert!(!old.exists(), "{:?}", old.parent().map(left));
        let dir = root.join(&moved);
        assert!(usable(std::env::consts::OS, &dir));
        assert!(crate::fuses::are_off(&std::fs::read(exe(&dir)).unwrap()).unwrap());
        assert!(!dir.join(".aio-used").exists() && !dir.join(FUSED).exists());
        assert_eq!(std::fs::read_to_string(dir.join(USED)).unwrap(), "2024-10-27T03:33:20.123Z\n");
        assert!(std::fs::read_to_string(dir.join(COMPLETE)).unwrap().starts_with("https://github.com/electron/"));
        assert_eq!(std::fs::read_to_string(root.join(&there).join(COMPLETE)).unwrap(), "aio's\n");
        // The live app's record is aio's now; the gone one's is not carried.
        let uses = crate::prune::uses(&root).unwrap();
        assert_eq!(uses.len(), 1);
        assert_eq!(
            uses[0].1.as_ref().map(|u| (u.exe.as_str(), u.version.as_str())),
            Some((&*live.to_string_lossy(), "45.0.1"))
        );
        let mut want = vec![moved.clone(), there.clone(), "apps".into()];
        want.sort();
        assert_eq!(left(&root), want);
    }

    /// A fake Electron executable: it holds a fuse wire, all on.
    fn fake_electron() -> Vec<u8> {
        [b"elf".as_slice(), crate::fuses::SENTINEL, &[1, 8], b"11111111"].concat()
    }

    /// A fake Electron zip for `slug` holding `files`, and a mirror serving
    /// it with its SHA-256 list, counting what it is asked.
    fn mirror_of(
        version: &str,
        slug: &str,
        files: &[(&str, &[u8])],
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let mut w = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        for (f, body) in files {
            w.start_file(*f, zip::write::SimpleFileOptions::default().unix_permissions(0o755)).unwrap();
            w.write_all(body).unwrap();
        }
        let zip = w.finish().unwrap().into_inner();
        let sums = format!("{} *electron-v{version}-{slug}.zip\n", aio::sha256::hex(&Sha256::digest(&zip)));
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mirror = format!("http://{}/", l.local_addr().unwrap());
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n = asked.clone();
        std::thread::spawn(move || {
            for mut s in l.incoming().flatten() {
                n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut req = [0; 4096];
                let got = s.read(&mut req).unwrap_or(0);
                let path = String::from_utf8_lossy(&req[..got]).split(' ').nth(1).unwrap_or("").to_string();
                let body = if path.ends_with("SHASUMS256.txt") { sums.as_bytes().to_vec() } else { zip.clone() };
                _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes());
                _ = s.write_all(&body);
            }
        });
        (mirror, asked)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_offline_build_bundles_the_runtime_once_fetched_trimmed_and_fused() {
        let bin = fake_electron();
        let (mirror, asked) = mirror_of(
            VERSION,
            "linux-arm64",
            &[
                ("electron", &bin),
                ("locales/en-US.pak", b"us"),
                ("locales/en-GB.pak", b"gb"),
                ("locales/de.pak", b"de"),
                ("libvulkan.so.1", b"vk"),
                ("libvk_swiftshader.so", b"ss"),
            ],
        );
        let t = tempfile::tempdir().unwrap();
        let (cache, into) = (t.path().join("cache"), t.path().join("bundle/electron"));
        let gone = bundle_from(&into, "linux-arm64", &cache, false, &mirror).unwrap();
        assert_eq!(gone, ["locales/de.pak"], "only the locales: the extras are kept unless asked");
        assert!(into.join("locales/en-US.pak").exists() && into.join("locales/en-GB.pak").exists());
        assert!(into.join("libvulkan.so.1").exists() && into.join(FUSED).exists());
        assert!(crate::fuses::are_off(&std::fs::read(exe(&into)).unwrap()).unwrap());
        assert!(carries_runtime(&t.path().join("bundle")) && !carries_runtime(t.path()));
        // Again, with the extras stripped: from the cache, nothing asked.
        let before = asked.load(std::sync::atomic::Ordering::SeqCst);
        let gone = bundle_from(&into, "linux-arm64", &cache, true, "http://127.0.0.1:1/").unwrap();
        assert_eq!(gone, ["locales/de.pak", "libvk_swiftshader.so", "libvulkan.so.1"]);
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), before);
        // A cached zip that is no longer the listed one is fetched again.
        let zip = cache.join(format!("v{VERSION}/electron-v{VERSION}-linux-arm64.zip"));
        std::fs::write(&zip, b"changed").unwrap();
        bundle_from(&into, "linux-arm64", &cache, false, &mirror).unwrap();
        assert!(std::fs::read(&zip).unwrap() != b"changed");
        let e = bundle_from(&into, "linux-riscv64", &cache, false, &mirror).unwrap_err();
        assert!(e.to_string().contains("not in SHASUMS256"), "{e}");
    }

    #[test]
    fn a_windows_runtime_is_bundled_from_any_os() {
        let bin = fake_electron();
        let (mirror, _) = mirror_of(
            VERSION,
            "win32-x64",
            &[
                ("electron.exe", &bin),
                ("locales/en-US.pak", b"us"),
                ("locales/fr.pak", b"fr"),
                ("vk_swiftshader.dll", b"ss"),
                ("dxcompiler.dll", b"dx"),
            ],
        );
        let t = tempfile::tempdir().unwrap();
        let into = t.path().join("electron");
        let gone = bundle_from(&into, "win32-x64", &t.path().join("cache"), true, &mirror).unwrap();
        assert_eq!(gone, ["locales/fr.pak", "dxcompiler.dll", "vk_swiftshader.dll"]);
        let exe = exe_for("windows", &into);
        assert!(crate::fuses::are_off(&std::fs::read(exe).unwrap()).unwrap() && into.join(FUSED).exists());
    }

    #[test]
    fn a_macos_runtime_keeps_english_lproj_locales_in_both_places() {
        let t = tempfile::tempdir().unwrap();
        let (app, fw) = (t.path().join(MAC_LOCALES[0]), t.path().join(MAC_LOCALES[1]));
        for l in ["en", "en_GB", "de", "pt_BR"] {
            std::fs::create_dir_all(app.join(format!("{l}.lproj"))).unwrap();
            std::fs::create_dir_all(fw.join(format!("{l}.lproj"))).unwrap();
            std::fs::write(fw.join(format!("{l}.lproj/locale.pak")), l).unwrap();
        }
        std::fs::write(fw.join("resources.pak"), "kept").unwrap();
        let mut gone = trim("macos", t.path(), false).unwrap();
        gone.sort();
        assert_eq!(gone.len(), 4, "{gone:?}");
        assert!(gone.iter().all(|g| g.ends_with("/de.lproj") || g.ends_with("/pt_BR.lproj")), "{gone:?}");
        for d in [&app, &fw] {
            assert!(d.join("en.lproj").is_dir() && d.join("en_GB.lproj").is_dir() && !d.join("de.lproj").exists());
        }
        assert!(fw.join("resources.pak").exists());
        // A layout without one of them trims what is there.
        std::fs::remove_dir_all(&app).unwrap();
        assert_eq!(trim("macos", t.path(), true).unwrap(), Vec::<String>::new());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_macos_runtime_is_bundled_on_a_mac_only() {
        let e = bundle_from(Path::new("unused"), "darwin-arm64", Path::new("unused"), false, "http://127.0.0.1:1/");
        assert!(e.unwrap_err().to_string().contains("takes a Mac"));
    }

    #[test]
    fn finds_the_listed_hash() {
        let a = "a".repeat(64);
        let sums = format!("{} *electron-v1-linux-arm64.zip\n{a} *electron-v1-linux-x64.zip\n", "b".repeat(64));
        assert_eq!(listed_sha(&sums, "electron-v1-linux-x64.zip"), Some(a));
        assert_eq!(listed_sha(&sums, "nope.zip"), None);
        assert_eq!(listed_sha("short *x.zip", "x.zip"), None);
    }

    #[test]
    fn the_store_is_aios() {
        let d = dir();
        if std::env::var_os("AIORT_RUNTIME").is_none() && bundled().is_none() {
            assert_eq!(d, root().join(format!("{VERSION}-{}", slug())));
        }
        if std::env::var_os("AIO_SHARED_ELECTRON_DIR").is_none() {
            assert!(root().ends_with(Path::new("aio").join("electron")), "{}", root().display());
        }
    }

    fn zip_with(names: &[&str]) -> io::Cursor<Vec<u8>> {
        let mut w = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        for n in names {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b"x").unwrap();
        }
        io::Cursor::new(w.finish().unwrap().into_inner())
    }

    #[test]
    fn installs_whole_or_not_at_all() {
        let t = std::env::temp_dir().join(format!("aio-rt-install-{}", std::process::id()));
        let dir = t.join("rt");
        assert!(install(zip_with(&["readme.txt"]), &dir).is_err());
        assert!(!dir.exists());
        assert_eq!(std::fs::read_dir(&t).unwrap().count(), 0, "no staging left behind");
        let name = exe(Path::new("")).to_string_lossy().into_owned();
        install(zip_with(&[&name]), &dir).unwrap();
        assert!(exe(&dir).exists());
        std::fs::remove_dir_all(&t).unwrap();
    }
}

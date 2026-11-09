//! A macOS app built away from a Mac: Rust compiles it, zig's `cc` (zig
//! 0.15.2, fetched once and checked by its SHA-256) compiles its C (SQLite,
//! ring) and links it. zig brings its own libSystem stubs; the two system
//! libraries an app links beyond them (CoreFoundation, for the time zone,
//! and libiconv) get stubs written here: the names of the functions used,
//! nothing of Apple's SDK. zig's linker signs the arm64 binary ad hoc, as
//! Apple Silicon requires. The bundle itself is not sealed: `codesign` runs
//! on a Mac only.
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub const VERSION: &str = "0.15.2";

/// zig's release archive per host (OS, arch) and its SHA-256 (ziglang.org's
/// download index for [`VERSION`]).
const RELEASES: &[(&str, &str, &str, &str)] = &[
    (
        "linux",
        "x86_64",
        "zig-x86_64-linux-0.15.2.tar.xz",
        "02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239",
    ),
    (
        "linux",
        "aarch64",
        "zig-aarch64-linux-0.15.2.tar.xz",
        "958ed7d1e00d0ea76590d27666efbf7a932281b3d7ba0c6b01b0ff26498f667f",
    ),
    (
        "windows",
        "x86_64",
        "zig-x86_64-windows-0.15.2.zip",
        "3a0ed1e8799a2f8ce2a6e6290a9ff22e6906f8227865911fb7ddedc3cc14cb0c",
    ),
    (
        "windows",
        "aarch64",
        "zig-aarch64-windows-0.15.2.zip",
        "b926465f8872bf983422257cd9ec248bb2b270996fbe8d57872cca13b56fc370",
    ),
];

/// The archive for `os`/`arch`: its URL, SHA-256 and top folder (pure).
pub fn release(os: &str, arch: &str) -> Option<(String, &'static str, &'static str)> {
    let (.., file, sha) = RELEASES.iter().find(|(o, a, ..)| *o == os && *a == arch)?;
    let dir = file.trim_end_matches(".tar.xz").trim_end_matches(".zip");
    Some((format!("https://ziglang.org/download/{VERSION}/{file}"), sha, dir))
}

/// zig's own name for the macOS `triple`, with the oldest macOS the app
/// runs on (the bundle's `LSMinimumSystemVersion`) (pure).
pub fn target(triple: &str) -> Option<&'static str> {
    match triple {
        "aarch64-apple-darwin" => Some("aarch64-macos.11.0"),
        "x86_64-apple-darwin" => Some("x86_64-macos.11.0"),
        _ => None,
    }
}

/// The compiler arguments rustc and cc-rs give, as zig's `cc` takes them:
/// their own target (`--target=arm64-apple-macosx`, `-target x`, `-arch x`)
/// left out, zig's is given (pure).
pub fn cc_args(args: &[OsString]) -> Vec<OsString> {
    let mut out = vec![];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.to_str() {
            Some("-target" | "-arch") => _ = it.next(),
            Some(s) if s.starts_with("--target=") => {}
            _ => out.push(a.clone()),
        }
    }
    out
}

/// `ax __zig <zig> <stubs> <target> args…`: zig's `cc` for that target,
/// the stubs' frameworks and libraries found (what the scripts [`env`]
/// writes run). `None`: ax isn't that.
pub fn hook() -> Option<ExitCode> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != "__zig" {
        return None;
    }
    let (Some(zig), Some(stubs), Some(target)) = (args.next(), args.next(), args.next()) else {
        eprintln!("ax __zig <zig> <stubs> <target> args…");
        return Some(ExitCode::from(2));
    };
    let rest: Vec<OsString> = args.collect();
    let mut cmd = std::process::Command::new(&zig);
    cmd.arg("cc").arg("-target").arg(target).arg("-F").arg(&stubs).arg("-L").arg(&stubs).args(cc_args(&rest));
    let e = crate::hotpatch::become_(cmd);
    eprintln!("ax: can't run {}: {e}", zig.to_string_lossy());
    Some(ExitCode::FAILURE)
}

/// CoreFoundation's functions an app's libraries call (iana-time-zone's
/// local time zone), as a text stub (Apple's `.tbd` form).
const CORE_FOUNDATION: &str = "--- !tapi-tbd
tbd-version:     4
targets:         [ x86_64-macos, arm64-macos ]
install-name:    '/System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation'
exports:
  - targets:         [ x86_64-macos, arm64-macos ]
    symbols:         [ _CFRelease, _CFStringGetBytes, _CFStringGetCStringPtr, _CFStringGetLength,
                       _CFTimeZoneCopySystem, _CFTimeZoneGetName, _CFTimeZoneResetSystem ]
...
";

/// libiconv, which Rust's standard library names on macOS.
const ICONV: &str = "--- !tapi-tbd
tbd-version:     4
targets:         [ x86_64-macos, arm64-macos ]
install-name:    '/usr/lib/libiconv.2.dylib'
exports:
  - targets:         [ x86_64-macos, arm64-macos ]
    symbols:         [ _iconv, _iconv_close, _iconv_open ]
...
";

/// `path` quoted for the script that runs it (pure).
fn quoted(path: &Path) -> String {
    let p = path.display().to_string();
    match cfg!(windows) {
        true => format!("\"{p}\""),
        false => format!("'{}'", p.replace('\'', r"'\''")),
    }
}

/// A script running `line` with the arguments it is given (pure).
pub fn script(line: &str) -> String {
    match cfg!(windows) {
        true => format!("@{line} %*\r\n"),
        false => format!("#!/bin/sh\nexec {line} \"$@\"\n"),
    }
}

/// This machine's zig release: its URL, SHA-256 and where aio keeps it
/// (beside the runtime, never on PATH).
fn this() -> Result<(String, &'static str, PathBuf), String> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let (url, sha, dir) = release(os, arch).ok_or_else(|| format!("no zig {VERSION} for {os} {arch}"))?;
    let to = aio::paths::tool_home().join("tools").join(dir).join(format!("zig{}", std::env::consts::EXE_SUFFIX));
    Ok((url, sha, to))
}

/// aio's zig, when it is here (nothing fetched).
pub fn here() -> Result<PathBuf, String> {
    Some(this()?.2).filter(|p| p.is_file()).ok_or_else(|| "not here; ax setup fetches it".into())
}

/// zig here: aio's own, fetched once (checked).
pub fn ensure() -> Result<PathBuf, String> {
    let (url, sha, to) = this()?;
    if !to.is_file() {
        eprintln!("ax: fetching zig {VERSION}, which links macOS apps away from a Mac…");
    }
    crate::setup::fetch_tool(&url, sha, &to, unpack).map(|()| to)
}

/// zig's release archive `archive`, whose top folder becomes `to`'s.
fn unpack(archive: &Path, to: &Path) -> Result<(), String> {
    let e = |e: std::io::Error| e.to_string();
    let home = to.parent().ok_or("no folder")?;
    let tools = home.parent().ok_or("no folder")?;
    let tmp = tools.join(format!(".zig-unpack-{}", std::process::id()));
    _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(e)?;
    let out = match cfg!(windows) {
        true => std::fs::File::open(archive)
            .map_err(e)
            .and_then(|f| zip::ZipArchive::new(f).map_err(|e| e.to_string()))
            .and_then(|mut z| z.extract(&tmp).map_err(|e| e.to_string())),
        false => match std::process::Command::new("tar").arg("-xJf").arg(archive).arg("-C").arg(&tmp).status() {
            Ok(s) if s.success() => Ok(()),
            _ => Err("tar -xJf failed: install xz (sudo apt install xz-utils)".into()),
        },
    };
    let moved = out.and_then(|()| {
        _ = std::fs::remove_dir_all(home);
        std::fs::rename(tmp.join(home.file_name().ok_or("no name")?), home).map_err(e)
    });
    _ = std::fs::remove_dir_all(&tmp);
    moved
}

/// What cargo builds macOS `triple` with here: its linker, its C compiler
/// and archiver (scripts beside zig, through this ax), `SDKROOT` the stubs
/// (so rustc looks for no Xcode). Each script is written anew: ax may
/// have moved.
pub fn env(triple: &str) -> Result<Vec<(String, OsString)>, String> {
    let target = target(triple).ok_or_else(|| format!("{triple} is no macOS target"))?;
    let zig = ensure()?;
    let dir = zig.parent().ok_or("no folder")?.join("ax-macos");
    let fw = dir.join("CoreFoundation.framework");
    let e = |e: std::io::Error| format!("{}: {e}", dir.display());
    std::fs::create_dir_all(&fw).map_err(e)?;
    std::fs::write(fw.join("CoreFoundation.tbd"), CORE_FOUNDATION).map_err(e)?;
    std::fs::write(dir.join("libiconv.tbd"), ICONV).map_err(e)?;
    let ax = std::env::current_exe().map_err(|e| e.to_string())?;
    let ext = if cfg!(windows) { ".cmd" } else { "" };
    let cc = dir.join(format!("cc-{triple}{ext}"));
    let ar = dir.join(format!("ar{ext}"));
    let cc_line = format!("{} __zig {} {} {target}", quoted(&ax), quoted(&zig), quoted(&dir));
    crate::setup::write_exe(script(&cc_line).as_bytes(), &cc)?;
    crate::setup::write_exe(script(&format!("{} ar", quoted(&zig))).as_bytes(), &ar)?;
    let t = triple.replace('-', "_");
    Ok(vec![
        (crate::platform::linker_var(triple), cc.clone().into()),
        (format!("CC_{t}"), cc.into()),
        (format!("AR_{t}"), ar.into()),
        ("SDKROOT".into(), dir.into()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rustc_and_cc_rs_targets_are_left_out_for_zigs_own() {
        let a = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        let given = a(&["--target=arm64-apple-macosx", "-O2", "-arch", "arm64", "-c", "x.c", "-target", "t", "-lm"]);
        assert_eq!(cc_args(&given), a(&["-O2", "-c", "x.c", "-lm"]));
        assert_eq!(cc_args(&a(&["-framework", "CoreFoundation"])), a(&["-framework", "CoreFoundation"]));
        assert_eq!(target("aarch64-apple-darwin"), Some("aarch64-macos.11.0"));
        assert_eq!(target("x86_64-apple-darwin"), Some("x86_64-macos.11.0"));
        assert_eq!(target("x86_64-unknown-linux-gnu"), None);
    }

    #[test]
    fn every_build_host_but_a_mac_has_a_pinned_zig() {
        for (os, arch) in [("linux", "x86_64"), ("linux", "aarch64"), ("windows", "x86_64"), ("windows", "aarch64")] {
            let (url, sha, dir) = release(os, arch).unwrap();
            assert!(url.starts_with("https://ziglang.org/download/0.15.2/zig-"), "{url}");
            assert!(url.ends_with(if os == "windows" { ".zip" } else { ".tar.xz" }), "{url}");
            assert!(url.contains(dir) && !dir.contains(".tar") && !dir.ends_with(".zip"), "{dir}");
            assert!(sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()), "{sha}");
        }
        assert_eq!(release("macos", "aarch64"), None, "a Mac links with its own tools");
    }

    #[test]
    fn the_scripts_pass_every_argument_on_and_quote_their_paths() {
        if cfg!(windows) {
            assert_eq!(script("\"C:\\a b\\ax.exe\" ar"), "@\"C:\\a b\\ax.exe\" ar %*\r\n");
        } else {
            assert_eq!(script("'/a b/zig' ar"), "#!/bin/sh\nexec '/a b/zig' ar \"$@\"\n");
            assert_eq!(quoted(Path::new("/it's/zig")), r"'/it'\''s/zig'");
        }
    }

    #[test]
    fn the_stubs_name_only_what_an_app_calls() {
        for stub in [CORE_FOUNDATION, ICONV] {
            assert!(stub.starts_with("--- !tapi-tbd\ntbd-version:     4\n") && stub.ends_with("...\n"));
            assert!(stub.contains("[ x86_64-macos, arm64-macos ]"));
        }
        assert!(CORE_FOUNDATION.contains("_CFTimeZoneCopySystem") && ICONV.contains("'/usr/lib/libiconv.2.dylib'"));
    }
}

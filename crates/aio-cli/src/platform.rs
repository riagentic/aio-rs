//! `[build] platforms` (`--platforms a,b`): the OS and CPU each desktop
//! target is built for, from one machine (aio's `build.platforms`). Each is a
//! Rust target; a cross build needs it installed and a linker for it, and
//! which is missing is said before cargo runs, with the command that adds
//! it. A macOS binary is linked by zig away from a Mac (`zig`, fetched on
//! the first such build); on a Mac by its own tools (both CPUs there).
//! Nothing declared: this machine.

/// One OS and CPU.
#[derive(Debug, PartialEq, Eq)]
pub struct Platform {
    /// Its name in `platforms` and in `--platforms`.
    pub name: &'static str,
    pub triple: &'static str,
    /// `std::env::consts::OS`'s word.
    pub os: &'static str,
    /// `std::env::consts::ARCH`'s word.
    pub arch: &'static str,
    /// The C compiler that links it from another machine (None: the
    /// toolchain's own, or zig's for a Mac).
    pub linker: Option<&'static str>,
    /// What installs that linker.
    pub get: &'static str,
}

pub const ALL: [Platform; 6] = [
    Platform {
        name: "linux",
        triple: "x86_64-unknown-linux-gnu",
        os: "linux",
        arch: "x86_64",
        linker: Some("x86_64-linux-gnu-gcc"),
        get: "apt install gcc-x86-64-linux-gnu",
    },
    Platform {
        name: "linux-arm64",
        triple: "aarch64-unknown-linux-gnu",
        os: "linux",
        arch: "aarch64",
        linker: Some("aarch64-linux-gnu-gcc"),
        get: "apt install gcc-aarch64-linux-gnu",
    },
    Platform {
        name: "windows",
        triple: "x86_64-pc-windows-gnu",
        os: "windows",
        arch: "x86_64",
        linker: Some("x86_64-w64-mingw32-gcc"),
        get: "apt install gcc-mingw-w64-x86-64",
    },
    Platform {
        name: "windows-arm64",
        triple: "aarch64-pc-windows-gnullvm",
        os: "windows",
        arch: "aarch64",
        linker: Some("aarch64-w64-mingw32-clang"),
        get: "llvm-mingw (github.com/mstorsjo/llvm-mingw) on PATH",
    },
    Platform { name: "macos", triple: "x86_64-apple-darwin", os: "macos", arch: "x86_64", linker: None, get: "" },
    Platform {
        name: "macos-arm64",
        triple: "aarch64-apple-darwin",
        os: "macos",
        arch: "aarch64",
        linker: None,
        get: "",
    },
];

/// The platform of `os` and `arch` (this machine's: `std::env::consts`).
pub fn of(os: &str, arch: &str) -> Option<&'static Platform> {
    ALL.iter().find(|p| p.os == os && p.arch == arch)
}

/// This machine.
pub fn host() -> Result<&'static Platform, String> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    of(os, arch).ok_or_else(|| format!("ax builds no desktop app on {os} {arch}"))
}

/// `names` resolved in order, once each, `host` being `host`; an unknown
/// one is refused naming the known. Pure.
pub fn resolve(names: &[&str], host: &'static Platform) -> Result<Vec<&'static Platform>, String> {
    let mut out: Vec<&'static Platform> = vec![];
    for n in names.iter().map(|n| n.trim()).filter(|n| !n.is_empty()) {
        if n == "all" {
            for p in &ALL {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
            continue;
        }
        let p = match n {
            "host" => host,
            _ => ALL.iter().find(|p| p.name == n).ok_or_else(|| {
                format!("no platform `{n}` (platforms: {}, host, or all)", ALL.map(|p| p.name).join(", "))
            })?,
        };
        if !out.contains(&p) {
            out.push(p);
        }
    }
    Ok(out)
}

/// `x64` / `arm64`: the CPU in file names.
pub fn arch_word(p: &Platform) -> &'static str {
    match p.arch {
        "aarch64" => "arm64",
        _ => "x64",
    }
}

/// Why `p` can't be built on `host`, if so: Rust's target missing
/// (`has_target`), or its linker (`has_tool`). Pure.
pub fn blocker(
    p: &Platform,
    host: &Platform,
    has_target: &dyn Fn(&str) -> bool,
    has_tool: &dyn Fn(&str) -> bool,
) -> Option<String> {
    if p == host {
        return None;
    }
    if !has_target(p.triple) {
        return Some(format!(
            "{}: Rust's {} target is not installed: rustup target add {}",
            p.name, p.triple, p.triple
        ));
    }
    // A Mac links Linux and Windows the same way, with the cross compiler.
    match p.linker {
        Some(l) if !has_tool(l) => Some(format!("{}: no {l} to link it: {}", p.name, p.get)),
        _ => None,
    }
}

/// The cargo variable naming the linker of `triple`.
pub fn linker_var(triple: &str) -> String {
    format!("CARGO_TARGET_{}_LINKER", triple.to_uppercase().replace('-', "_"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(n: &str) -> &'static Platform {
        ALL.iter().find(|p| p.name == n).unwrap()
    }

    #[test]
    fn platforms_resolve_in_order_once_with_host_and_an_unknown_is_refused() {
        let host = named("linux");
        let names = |ps: Vec<&Platform>| ps.iter().map(|p| p.name).collect::<Vec<_>>();
        assert_eq!(
            names(resolve(&["windows", " host ", "linux", "", "macos-arm64"], host).unwrap()),
            ["windows", "linux", "macos-arm64"]
        );
        let e = resolve(&["linux", "solaris", "win"], host).unwrap_err();
        assert!(e.starts_with("no platform `solaris` (platforms: linux, linux-arm64, windows,"), "{e}");
        assert!(e.ends_with("macos-arm64, host, or all)"), "{e}");
        // aio's --all-platforms: every one, once, the ones named first first.
        assert_eq!(names(resolve(&["macos", "all"], host).unwrap()).len(), ALL.len());
        assert_eq!(resolve(&["macos", "all"], host).unwrap()[0].name, "macos");
        assert_eq!(of("linux", "aarch64"), Some(named("linux-arm64")));
        assert_eq!(of("macos", "aarch64").map(arch_word), Some("arm64"));
        assert_eq!(of("freebsd", "x86_64"), None);
        assert!(super::host().is_ok(), "this machine builds");
    }

    #[test]
    fn what_a_cross_build_lacks_is_said_with_what_adds_it() {
        let (linux, mac) = (named("linux"), named("macos-arm64"));
        let all = |_: &str| true;
        let none = |_: &str| false;
        assert_eq!(blocker(linux, linux, &none, &none), None, "the host needs nothing");
        assert_eq!(blocker(named("windows"), linux, &all, &all), None);
        let e = blocker(named("linux-arm64"), linux, &none, &all).unwrap();
        assert!(e.ends_with("rustup target add aarch64-unknown-linux-gnu"), "{e}");
        let e = blocker(named("linux-arm64"), linux, &all, &none).unwrap();
        assert_eq!(e, "linux-arm64: no aarch64-linux-gnu-gcc to link it: apt install gcc-aarch64-linux-gnu");
        assert_eq!(blocker(mac, linux, &all, &none), None, "zig links a Mac's binary anywhere");
        assert!(blocker(mac, linux, &none, &all).unwrap().ends_with("rustup target add aarch64-apple-darwin"));
        assert_eq!(blocker(named("macos"), mac, &all, &all), None, "a Mac builds both CPUs");
        assert!(blocker(named("macos"), mac, &none, &all).unwrap().ends_with("rustup target add x86_64-apple-darwin"));
        assert!(blocker(named("windows"), mac, &all, &none).unwrap().contains("x86_64-w64-mingw32-gcc"));
        assert_eq!(linker_var("aarch64-unknown-linux-gnu"), "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER");
    }
}

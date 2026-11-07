//! The toolchain ax builds with is the one its build folder is named by:
//! the workspace root's, wherever in the workspace ax runs and wherever its
//! cargo runs (rustup picks by folder: a member's own toolchain file is not
//! the workspace's). Unix: ci-windows.sh runs tests cross-built, without
//! the binary beside them.
#![cfg(unix)]
use std::process::Command;

#[test]
fn a_members_own_toolchain_file_is_not_the_builds() {
    let d = tempfile::tempdir().unwrap();
    let ws = d.path().join("ws");
    std::fs::create_dir_all(ws.join("m/src")).unwrap();
    // The root pins the toolchain this test runs on; the member one no machine has.
    let active = Command::new("rustup").args(["show", "active-toolchain"]).output().unwrap();
    let here = String::from_utf8_lossy(&active.stdout).split_whitespace().next().unwrap().to_string();
    std::fs::write(ws.join("Cargo.toml"), "[workspace]\nmembers = [\"m\"]\nresolver = \"3\"\n").unwrap();
    std::fs::write(ws.join("rust-toolchain.toml"), format!("[toolchain]\nchannel = \"{here}\"\n")).unwrap();
    std::fs::write(ws.join("m/Cargo.toml"), "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2024\"\n")
        .unwrap();
    std::fs::write(ws.join("m/src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(ws.join("m/rust-toolchain.toml"), "[toolchain]\nchannel = \"nightly-2099-01-01\"\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ax"))
        .args(["dev", "m", "--warm"])
        .current_dir(ws.join("m"))
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("CARGO")
        .env("RUSTUP_AUTO_INSTALL", "0")
        .env("AIORT_TARGET_DIR", d.path().join("target"))
        .env("HOME", d.path())
        .env("XDG_CACHE_HOME", d.path().join("cache"))
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{said}");
    assert!(!said.contains("nightly-2099"), "{said}");
}

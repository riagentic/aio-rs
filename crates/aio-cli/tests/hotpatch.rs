//! ax dev's own hot-patching (`src/hotpatch.rs`), end to end: a fixture
//! workspace (a library and an app on subsecond) built fat, run, and
//! patched three times while it runs: twice in the app's crate, once in
//! the library; the app prints the new values without a restart, and the
//! patched code sees the thread-locals the running binary counted (Linux
//! x86_64; on macOS a patch has its own, new, as with dx).
#![cfg(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos"))]

#[allow(dead_code)]
#[path = "../src/hotpatch.rs"]
mod hotpatch;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const APP: &str = r#"use std::cell::Cell;

thread_local! {
    static TICKS: Cell<u32> = const { Cell::new(0) };
}

fn value() -> u32 {
    1
}

// Read by each patch's own copy of this function: the app's thread-locals
// (this crate's and the library's), as the running binary counted them.
fn ticks() -> u32 {
    TICKS.get() + 100 * fixture_lib::SEEN.get()
}

fn main() {
    println!("aslr {}", subsecond::aslr_reference());
    let mut line = String::new();
    loop {
        TICKS.set(TICKS.get() + 1);
        fixture_lib::SEEN.set(fixture_lib::SEEN.get() + 1);
        let app = subsecond::HotFn::current(value).call(());
        let lib = subsecond::HotFn::current(fixture_lib::value).call(());
        let ticks = subsecond::HotFn::current(ticks).call(());
        println!("value {app} {lib} {ticks}");
        line.clear();
        if std::io::stdin().read_line(&mut line).unwrap() == 0 {
            break;
        }
        let table = std::fs::read_to_string(line.trim()).unwrap();
        let table: subsecond::JumpTable = serde_json::from_str(&table).unwrap();
        unsafe { subsecond::apply_patch(table).unwrap() };
    }
}
"#;

const LIB: &str = "thread_local! {\n    pub static SEEN: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };\n}\n\npub fn value() -> u32 {\n    10\n}\n";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The fixture workspace in `dir`, its versions locked as aio's.
fn fixture(dir: &Path) {
    write(&dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\", \"lib\"]\nresolver = \"2\"\n");
    write(
        &dir.join("app/Cargo.toml"),
        "[package]\nname = \"fixture-app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n\
         fixture-lib = { path = \"../lib\" }\nsubsecond = \"=0.7.10\"\nserde_json = \"1\"\n",
    );
    write(&dir.join("app/src/main.rs"), APP);
    write(&dir.join("lib/Cargo.toml"), "[package]\nname = \"fixture-lib\"\nversion = \"0.1.0\"\nedition = \"2024\"\n");
    write(&dir.join("lib/src/lib.rs"), LIB);
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"), dir.join("Cargo.lock")).unwrap();
}

fn host() -> String {
    let out = Command::new("rustc").arg("-vV").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.lines().find_map(|l| l.strip_prefix("host: ")).unwrap().to_string()
}

fn edit(file: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(file).unwrap();
    assert!(text.contains(from), "{from} in {}", file.display());
    std::fs::write(file, text.replacen(from, to, 1)).unwrap();
}

#[test]
fn a_running_app_takes_patches_of_its_crate_and_of_a_library() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    fixture(&ws);
    let profile = "desktop-dev";
    let req = hotpatch::Request {
        dir: ws.join("app"),
        package: "fixture-app".into(),
        bin: "fixture-app".into(),
        // ax dev's layout (`dev::layout()`).
        cargo_args: [
            "--offline",
            "--config",
            &format!("profile.{profile}.strip=false"),
            "--config",
            &format!("profile.{profile}.inherits=\"dev\""),
            "--profile",
            profile,
            "--target",
            &host(),
        ]
        .map(String::from)
        .into(),
        envs: vec![("CARGO_TARGET_DIR".into(), dir.path().join("target").display().to_string())],
        ax: PathBuf::from(env!("CARGO_BIN_EXE_ax")),
    };
    let mut fat = hotpatch::fat_build(&req).unwrap();
    eprintln!("fat build: cargo {:?}, link {:?}", fat.build, fat.link);

    let mut app = Command::new(&fat.exe).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = app.stdin.take().unwrap();
    let mut lines = BufReader::new(app.stdout.take().unwrap()).lines();
    let mut next = || lines.next().unwrap().unwrap();
    let aslr: u64 = next().strip_prefix("aslr ").unwrap().parse().unwrap();
    assert_eq!(next(), "value 1 10 101");

    let main_rs = ws.join("app/src/main.rs");
    let lib_rs = ws.join("lib/src/lib.rs");
    // What the patched `ticks` reads after `n` turns of the loop.
    let ticks = |n: u32| if cfg!(target_os = "linux") { 101 * n } else { 0 };
    let steps = [
        (&main_rs, "    1\n", "    2\n", format!("value 2 10 {}", ticks(2))),
        (&main_rs, "    2\n", "    3\n", format!("value 3 10 {}", ticks(3))),
        (&lib_rs, "    10\n", "    20\n", format!("value 3 20 {}", ticks(4))),
    ];
    for (i, (file, from, to, want)) in steps.into_iter().enumerate() {
        edit(file, from, to);
        let patch = hotpatch::patch(&mut fat, aslr, std::slice::from_ref(file)).unwrap();
        let t = patch.times;
        eprintln!(
            "patch {}: compile {:?}, stub {:?}, link {:?}, total {:?}",
            i + 1,
            t.compile,
            t.stub,
            t.link,
            t.total
        );
        let table = dir.path().join(format!("table-{i}.json"));
        std::fs::write(&table, serde_json::to_string(&patch.table).unwrap()).unwrap();
        writeln!(stdin, "{}", table.display()).unwrap();
        assert_eq!(next(), want, "after patch {}", i + 1);
    }
    drop(stdin);
    assert!(app.wait().unwrap().success());

    // The app crate's temporary bitcode, objects (macOS keeps those: its
    // debug info) and rustc's folders (`-Csave-temps`) are gone once linked.
    let deps = fat.exe.with_file_name("deps");
    let temps: Vec<_> = std::fs::read_dir(&deps)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            let app = n.starts_with("fixture_app-") && n.contains(".rcgu.");
            (app && !(cfg!(target_os = "macos") && n.ends_with(".rcgu.o"))) || (n.starts_with("rmeta") && n.len() == 11)
        })
        .collect();
    assert!(temps.is_empty(), "{temps:?}");

    // A file no crate builds from (here the manifest): a rebuild's.
    let err = hotpatch::patch(&mut fat, aslr, &[ws.join("app/Cargo.toml")]).err().unwrap();
    assert!(err.contains("no crate of the app builds from"), "{err}");
}

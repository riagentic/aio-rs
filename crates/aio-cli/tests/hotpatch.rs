//! ax dev's own hot-patching (`src/hotpatch.rs`), end to end: a fixture
//! workspace (a library, a library on it, and an app on aio-rs-hotpatch)
//! built fat, run, and patched while it runs: in the app's crate and in
//! the first library; the app prints the new values without a restart. On Linux
//! x86_64 and Windows the patched code sees the running binary's
//! thread-locals and statics (an `AtomicU32` and a `OnceLock` set before
//! the patch), while a static the patch adds is its own; a static the patch
//! adds is its own on every platform; a backtrace taken in patched code
//! names its function, as one in the fat binary does. On macOS a patch has its own
//! thread-locals and statics. Then the same edit rebuilt, the
//! path without hot-patching, for its time.
#![cfg(any(
    all(target_os = "linux", target_arch = "x86_64"),
    target_os = "macos",
    all(windows, target_arch = "x86_64")
))]

#[allow(dead_code)]
#[path = "../src/hotpatch.rs"]
mod hotpatch;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const APP: &str = r#"use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

thread_local! {
    static TICKS: Cell<u32> = const { Cell::new(0) };
}

static HITS: AtomicU32 = AtomicU32::new(0);
static NAME: OnceLock<String> = OnceLock::new();

fn value() -> u32 {
    1
}

// The library's generic, compiled into the library on it: a patch of the
// library alone changes it only once that one is built again (`finish`).
// Its copy here (of no value: 0) is this crate's or the library's.
fn doubled() -> u32 {
    fixture_mid::doubled() + fixture_lib::twice(0u8)
}

// Read by each patch's own copy of this function: the app's thread-locals
// (this crate's and the library's), as the running binary counted them.
fn ticks() -> u32 {
    TICKS.get() + 100 * fixture_lib::SEEN.get()
}

// Likewise the app's statics: the running counter and the name set once.
fn stats() -> String {
    format!("{} {}", HITS.load(Ordering::Relaxed), NAME.get().map(|s| s.as_str()).unwrap_or("?"))
}

// A backtrace taken here (resolved as a panic's is) names this function:
// once patched to take one, `TRACE`; 0 when it doesn't; 1 before.
const TRACE: u32 = 0;
fn named() -> u32 {
    if TRACE == 0 {
        return 1;
    }
    let bt = std::backtrace::Backtrace::force_capture().to_string();
    if bt.lines().any(|l| l.trim_end().ends_with(": fixture_app::named")) { TRACE } else { 0 }
}

// Windows (MSVC), as aio-shell does after each patch (`refresh_symbols`):
// std's backtrace resolver (dbghelp) lists the modules again, under its lock.
#[cfg(all(windows, target_env = "msvc"))]
fn refresh_symbols() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleA(name: *const u8) -> usize;
        fn GetProcAddress(module: usize, name: *const u8) -> usize;
        fn CreateMutexA(attributes: usize, owned: i32, name: *const u8) -> usize;
        fn WaitForSingleObject(handle: usize, ms: u32) -> u32;
        fn ReleaseMutex(handle: usize) -> i32;
        fn CloseHandle(handle: usize) -> i32;
        fn GetCurrentProcessId() -> u32;
    }
    // SAFETY: kernel32 calls as declared; SymRefreshModuleList is looked up by
    // name in dbghelp and called with its signature, under std's backtrace
    // mutex as std itself takes it; the C strings are NUL-terminated.
    unsafe {
        let module = GetModuleHandleA(b"dbghelp.dll\0".as_ptr());
        let f = if module == 0 { 0 } else { GetProcAddress(module, b"SymRefreshModuleList\0".as_ptr()) };
        if f == 0 {
            return;
        }
        let refresh: unsafe extern "system" fn(isize) -> i32 = std::mem::transmute(f);
        let name = format!("Local\\RustBacktraceMutex{:08X}\0", GetCurrentProcessId());
        let lock = CreateMutexA(0, 0, name.as_ptr());
        WaitForSingleObject(lock, u32::MAX);
        refresh(-1);
        ReleaseMutex(lock);
        CloseHandle(lock);
    }
}

#[cfg(not(all(windows, target_env = "msvc")))]
fn refresh_symbols() {}

// Windows: where the next patch DLL can load (as aio-shell's `Aio.aslr`
// answers before each patch): a free 256 MiB span past `main`, within
// 1.5 GiB of it; each patch loaded takes one.
#[cfg(windows)]
fn base() -> usize {
    #[repr(C)]
    struct Mbi { base: usize, allocation: usize, protect0: u32, partition: u16, size: usize, state: u32, protect: u32, kind: u32 }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn VirtualQuery(at: usize, out: *mut Mbi, len: usize) -> usize;
    }
    let main = aio_hotpatch::aslr_reference();
    let mut at = main;
    while at < main + (3 << 29) {
        // SAFETY: Mbi is a plain C struct: all-zero is valid.
        let mut m: Mbi = unsafe { std::mem::zeroed() };
        // SAFETY: a query of this process's own address space into a local of the size given.
        if unsafe { VirtualQuery(at, &mut m, std::mem::size_of::<Mbi>()) } == 0 {
            break;
        }
        let start = (at.max(m.base) + 0xffff) & !0xffff;
        if m.state == 0x10000 && start + (256 << 20) <= m.base + m.size {
            return start;
        }
        at = m.base + m.size;
    }
    0
}

#[cfg(not(windows))]
fn base() -> usize {
    0
}

fn main() {
    println!("aslr {}", aio_hotpatch::aslr_reference());
    let mut line = String::new();
    loop {
        TICKS.set(TICKS.get() + 1);
        fixture_lib::SEEN.set(fixture_lib::SEEN.get() + 1);
        HITS.fetch_add(1, Ordering::Relaxed);
        let _ = NAME.set("app".to_string());
        let app = aio_hotpatch::HotFn::current(value).call(());
        let lib = aio_hotpatch::HotFn::current(fixture_mid::value).call(());
        let ticks = aio_hotpatch::HotFn::current(ticks).call(());
        let stats = aio_hotpatch::HotFn::current(stats).call(());
        let named = aio_hotpatch::HotFn::current(named).call(());
        let doubled = aio_hotpatch::HotFn::current(doubled).call(());
        println!("value {app} {lib} {ticks} {stats} {named} {doubled}");
        println!("base {}", base());
        line.clear();
        if std::io::stdin().read_line(&mut line).unwrap() == 0 {
            break;
        }
        let table = std::fs::read_to_string(line.trim()).unwrap();
        let table: aio_hotpatch::JumpTable = serde_json::from_str(&table).unwrap();
        // SAFETY: the table is ax dev's patch of this very test binary.
        unsafe { aio_hotpatch::apply_patch(table).unwrap() };
        refresh_symbols();
    }
}
"#;

const LIB: &str = "thread_local! {\n    pub static SEEN: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };\n}\n\npub fn twice<T: Into<u32>>(t: T) -> u32 {\n    t.into() * 2\n}\n\npub fn value() -> u32 {\n    10\n}\n";

// The library on the first: a call of its function (the app's way to it,
// linked from the running copy when only the first library is replayed)
// and its generic, compiled in here.
const MID: &str = "pub fn value() -> u32 {\n    fixture_lib::value()\n}\n\npub fn doubled() -> u32 {\n    fixture_lib::twice(1u8)\n}\n";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The fixture workspace in `dir`, its versions locked as aio's.
fn fixture(dir: &Path) {
    write(&dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\", \"lib\", \"mid\"]\nresolver = \"2\"\n");
    let hotpatch = concat!(env!("CARGO_MANIFEST_DIR"), "/../hotpatch");
    write(
        &dir.join("app/Cargo.toml"),
        &format!(
            "[package]\nname = \"fixture-app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n\
             fixture-lib = {{ path = \"../lib\" }}\nfixture-mid = {{ path = \"../mid\" }}\nserde_json = \"1\"\n\
             aio-hotpatch = {{ package = \"aio-rs-hotpatch\", path = {hotpatch:?} }}\n"
        ),
    );
    write(&dir.join("app/src/main.rs"), APP);
    write(&dir.join("lib/Cargo.toml"), "[package]\nname = \"fixture-lib\"\nversion = \"0.1.0\"\nedition = \"2024\"\n");
    write(&dir.join("lib/src/lib.rs"), LIB);
    write(
        &dir.join("mid/Cargo.toml"),
        "[package]\nname = \"fixture-mid\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [dependencies]\nfixture-lib = { path = \"../lib\" }\n",
    );
    write(&dir.join("mid/src/lib.rs"), MID);
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"), dir.join("Cargo.lock")).unwrap();
}

/// The fixture's target: rustc's host, or `AIO_HOTPATCH_TARGET` (CI's
/// windows-gnu lane on an MSVC host).
fn host() -> String {
    if let Ok(t) = std::env::var("AIO_HOTPATCH_TARGET") {
        return t;
    }
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
    let request = |low: bool| hotpatch::Request {
        dir: ws.join("app"),
        package: "fixture-app".into(),
        bin: "fixture-app".into(),
        // ax dev's layout (`dev::layout()`): no --target, so the binary is
        // `<target>/desktop-dev/<bin>`; CI's windows-gnu lane names its target.
        cargo_args: [
            "--offline",
            "--config",
            &format!("profile.{profile}.strip=false"),
            "--config",
            &format!("profile.{profile}.inherits=\"dev\""),
            "--config",
            &format!("profile.{profile}.debug-assertions=false"),
            "--config",
            &format!("profile.{profile}.overflow-checks=false"),
            "--profile",
            profile,
        ]
        .into_iter()
        .map(String::from)
        .chain(std::env::var("AIO_HOTPATCH_TARGET").into_iter().flat_map(|t| ["--target".to_string(), t]))
        .collect(),
        envs: vec![("CARGO_TARGET_DIR".into(), dir.path().join("target").display().to_string())],
        ax: PathBuf::from(env!("CARGO_BIN_EXE_ax")),
        stop: Default::default(),
        low,
        sources: vec![ws.join("app"), ws.join("lib"), ws.join("mid")],
    };
    let req = request(false);
    let mut fat = hotpatch::fat_build(&req).unwrap();
    eprintln!("fat build: cargo {:?}, link {:?}", fat.build, fat.link);

    let mut app = Command::new(&fat.exe).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = app.stdin.take().unwrap();
    let mut lines = BufReader::new(app.stdout.take().unwrap()).lines();
    let mut next = || lines.next().unwrap().unwrap();
    // On Linux x86_64 and Windows a patch keeps the running thread-locals
    // and statics; on macOS a patch gets its own, so patched reads start fresh.
    let live = !cfg!(target_os = "macos");
    let ticks = |n: u32| if live { 101 * n } else { 0 };
    let stats = |hits: u32| if live { format!("{hits} app") } else { "0 ?".to_string() };

    let aslr: u64 = next().strip_prefix("aslr ").unwrap().parse().unwrap();
    // After each loop the app names where the next patch can load (Windows).
    let free = |line: String| cfg!(windows).then(|| line.strip_prefix("base ").unwrap().parse::<u64>().unwrap());
    // Before any patch the app itself runs: the real values on every
    // platform (one loop: TICKS 1 + 100*SEEN 1 = 101, HITS 1, NAME set).
    // Only a patched function's view then depends on the platform.
    assert_eq!(next(), "value 1 10 101 1 app 1 2");
    let mut base = free(next());

    let main_rs = ws.join("app/src/main.rs");
    let lib_rs = ws.join("lib/src/lib.rs");
    // The patched lib `value` uses a static the patch adds (not in the fat
    // binary): its own, on every platform, so the result is 20.
    let new_static = "    static NEW: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(20);\n    \
                      NEW.load(std::sync::atomic::Ordering::Relaxed)\n";
    // A later patch's frames too with dbghelp (MSVC); std's other resolver
    // lists the loaded libraries once, at its first backtrace, so a library
    // loaded after it is a stranger to it (Linux, macOS, MinGW: `<unknown>`).
    let later = if host().ends_with("-msvc") { 3 } else { 0 };
    // Each edit, the value it shows (`T`, `S`: the ticks and statics after
    // as many loops as tables sent), and whether the app crate's replay
    // (`finish`) patches again, with the value then: a library's edit is
    // patched alone first (the library on it owed too), and again only when
    // the code of either changed with it (the generic the library on it
    // compiles in). `None`: either (the
    // library's first static may move which crate's copy of an atomic
    // load the app crate calls: rustc's choice, by platform).
    let steps = [
        // A library's edit first: no app crate's objects (they'd pull its
        // code from the archive) in the patch, its objects all linked, and
        // the caller the app reaches it by, from the library on it.
        (&lib_rs, "    10\n", "    11\n", "value 1 11 T S 1 2", Some(None)),
        (&main_rs, "    1\n", "    2\n", "value 2 11 T S 1 2", Some(None)),
        (&main_rs, "    2\n", "    3\n", "value 3 11 T S 1 2", Some(None)),
        (&lib_rs, "    11\n", new_static, "value 3 20 T S 1 2", None),
        // A library edit nothing else compiles any of: one patch.
        (&lib_rs, "new(20)", "new(30)", "value 3 30 T S 1 2", Some(None)),
        // A backtrace in patched code names its function: the first one
        // resolved, and one after a later patch.
        (&main_rs, "TRACE: u32 = 0", "TRACE: u32 = 2", "value 3 30 T S 2 2", Some(None)),
        (&main_rs, "TRACE: u32 = 2", "TRACE: u32 = 3", "value 3 30 T S L 2", Some(None)),
        // The generic the library on it compiles in: its second patch carries it.
        (&lib_rs, "* 2", "* 3", "value 3 30 T S L 2", Some(Some("value 3 30 T S L 3"))),
        // The library compiles that generic's copy itself (the library
        // on it may share it then), and an edit drops it with a change the
        // app reaches through the library on it: that one's running
        // objects (and the app crate's) use the copy gone, so they are
        // built with it, not owed.
        (&lib_rs, "Relaxed)\n", "Relaxed) + 0 * twice(2u8)\n", "value 3 30 T S L 3", None),
        (&lib_rs, "+ 0 * twice(2u8)", "+ 3", "value 3 33 T S L 3", Some(None)),
    ];
    let mut loops = 1;
    let mut shown = |want: &str| {
        loops += 1;
        let (t, s) = (ticks(loops).to_string(), stats(loops));
        want.replace('T', &t).replace('S', &s).replace('L', &later.to_string())
    };
    let mut send = |table: &hotpatch::JumpTable, n: String| {
        let file = dir.path().join(format!("table-{n}.json"));
        std::fs::write(&file, serde_json::to_string(table).unwrap()).unwrap();
        writeln!(stdin, "{}", file.display()).unwrap();
    };
    for (i, (file, from, to, want, then)) in steps.into_iter().enumerate() {
        edit(file, from, to);
        let patch = hotpatch::patch(&mut fat, aslr, base, std::slice::from_ref(file)).unwrap();
        let t = patch.times;
        eprintln!(
            "patch {}: compile {:?}, stub {:?}, link {:?}, total {:?}",
            i + 1,
            t.compile,
            t.stub,
            t.link,
            t.total
        );
        send(&patch.table, i.to_string());
        assert_eq!(next(), shown(want), "after patch {}", i + 1);
        base = free(next());
        let again = hotpatch::finish(&mut fat, aslr, base).unwrap();
        if let Some(then) = then {
            assert_eq!(again.is_some(), then.is_some(), "patch {}: the app crate patched again", i + 1);
        }
        if let Some(p) = again {
            send(&p.table, format!("{i}-app"));
            let then = then.flatten().unwrap_or(want);
            assert_eq!(next(), shown(then), "after patch {} and its app crate", i + 1);
            base = free(next());
        }
    }

    // The library's rlib is the patch's: cargo builds it again next time.
    let fingerprints = fat.exe.with_file_name(".fingerprint");
    let lib_fresh =
        std::fs::read_dir(&fingerprints).unwrap().flatten().any(|e| e.path().join("lib-fixture_lib").is_file());
    assert!(!lib_fresh, "the patched library's fingerprint is left for cargo to trust");

    // The next start's build brought up to date behind the running app (at
    // low priority, as ax dev does when it is quiet): the app takes patches
    // on, its crate's patched code kept, and the next start builds nothing.
    assert!(hotpatch::refresh(&request(true)).unwrap(), "behind the patches: built");
    edit(&lib_rs, "new(30)", "new(40)");
    let patch = hotpatch::patch(&mut fat, aslr, base, std::slice::from_ref(&lib_rs)).unwrap();
    send(&patch.table, "after-refresh".into());
    assert_eq!(next(), shown("value 3 43 T S L 3"), "a patch after the refresh");
    base = free(next());
    if let Some(p) = hotpatch::finish(&mut fat, aslr, base).unwrap() {
        send(&p.table, "after-refresh-app".into());
        assert_eq!(next(), shown("value 3 43 T S L 3"), "its app crate");
    }
    assert!(hotpatch::refresh(&request(true)).unwrap(), "behind that patch: built");
    assert!(!hotpatch::refresh(&request(true)).unwrap(), "up to date: nothing to build");
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

    // The next start: the refreshed binary as it is, nothing built or linked.
    let next_start = hotpatch::fat_build(&req).unwrap();
    assert!(next_start.kept, "the next start builds nothing");
    drop(next_start);

    // A start after an edit made with no session running (a pull): the
    // kept build as it is, without cargo, with the sources it was built
    // from; the edit compiled before the app runs, linked once it says
    // where it is loaded, and the fresh process runs it.
    edit(&main_rs, "    3\n", "    5\n");
    let target = dir.path().join("target");
    let (mut last, built) = hotpatch::last(&target, &req).expect("the kept build, whole");
    assert!(built[&main_rs].contains("    3\n"), "the sources the kept build read");
    let compiled = hotpatch::compile(&mut last, std::slice::from_ref(&main_rs)).unwrap();
    let mut started = Command::new(&last.exe).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = started.stdin.take().unwrap();
    let mut lines = BufReader::new(started.stdout.take().unwrap()).lines();
    let mut next = || lines.next().unwrap().unwrap();
    let aslr_now: u64 = next().strip_prefix("aslr ").unwrap().parse().unwrap();
    assert!(next().starts_with("value 3 "), "as built");
    let base_now = free(next());
    let patch = hotpatch::link_patch(&mut last, compiled, aslr_now, base_now).unwrap();
    let file = dir.path().join("table-start.json");
    std::fs::write(&file, serde_json::to_string(&patch.table).unwrap()).unwrap();
    writeln!(stdin, "{}", file.display()).unwrap();
    assert!(next().starts_with("value 5 "), "the edit, patched in at the start");
    drop(stdin);
    assert!(started.wait().unwrap().success());
    edit(&main_rs, "    5\n", "    3\n");

    // Code that doesn't compile: the compiler's errors, for the person to
    // fix (ax dev tries no other build); the fat build stays as it was.
    edit(&main_rs, "    3\n", "    3 +\n");
    match hotpatch::patch(&mut fat, aslr, base, std::slice::from_ref(&main_rs)) {
        Err(hotpatch::Refused::Code(e)) => assert!(e.contains("error"), "{e}"),
        other => panic!("a compile error is the code's: {:?}", other.map(|_| ())),
    }
    edit(&main_rs, "    3 +\n", "    3\n");

    // A file no crate builds from (here the manifest): a rebuild's.
    let err = hotpatch::patch(&mut fat, aslr, base, &[ws.join("app/Cargo.toml")]).err().unwrap().to_string();
    assert!(err.contains("no crate of the app builds from"), "{err}");
    // A temporary copy gone by the time the burst settled (`sed -i`'s): nothing to patch.
    let gone = ws.join("app/src/sedAbC123");
    assert_eq!(hotpatch::patch(&mut fat, aslr, base, &[gone]).err(), Some(hotpatch::Refused::Unchanged));

    // The path without hot-patching, for its time: the same kind of edit
    // rebuilt as `ax dev` rebuilds (then it restarts the app).
    edit(&main_rs, "    3\n", "    4\n");
    let t = std::time::Instant::now();
    let built = Command::new("cargo")
        .args(["build", "-p", "fixture-app"])
        .args(&req.cargo_args)
        .current_dir(&req.dir)
        .envs(req.envs.iter().cloned())
        .status()
        .unwrap();
    assert!(built.success());
    eprintln!("rebuild: {:?}", t.elapsed());
}

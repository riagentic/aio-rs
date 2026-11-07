//! `ax dev`'s splash: a window within a moment of `ax dev`, while its first
//! build runs (cargo's own N/M and the crates it is on), a failed build's
//! errors on it, gone once the app's window is up. Tooling only: its own
//! Electron process on the shared runtime, never part of the app (the app's
//! build, its window and its behaviour are as without it).
//! `AIORT_NO_SPLASH=1` (or no runtime yet) leaves it out; `0` is as unset.
use std::io::Write;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

/// The splash's main process (Electron runs it as its app).
const MAIN_JS: &str = include_str!("dev_splash.js");

/// A build that ends sooner than this never shows the splash (a warm
/// start is a launch): it is started hidden, shown after it.
pub const SHOW_AFTER: Duration = Duration::from_millis(150);

struct Open {
    child: Child,
    stdin: ChildStdin,
}

static SPLASH: Mutex<Option<Open>> = Mutex::new(None);

/// The build it is up for has ended (built or failed): a hidden splash
/// is then not shown for it.
static ENDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Something to wait for: cargo compiling a crate, or a download. A build of
/// nothing (the last fat binary reused) shows no splash however long it takes.
static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn with<R>(f: impl FnOnce(&mut Option<Open>) -> R) -> R {
    f(&mut SPLASH.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Is a splash up (or starting)?
pub fn active() -> bool {
    with(|s| s.is_some())
}

/// Start the splash for `title`, on the display the app's window will use
/// (`put`: the app's own environment), hidden until [`SHOW_AFTER`].
pub fn open(title: &str, put: &dyn Fn(&mut Command), dir: &std::path::Path) {
    if crate::dev::env_on(std::env::var("AIORT_NO_SPLASH").ok().as_deref()) || active() {
        return;
    }
    let rt = aio_shell::runtime::dir();
    if !aio_shell::runtime::usable(std::env::consts::OS, &rt) {
        return;
    }
    let js = dir.join("dev-splash.js");
    if std::fs::create_dir_all(dir).is_err() || std::fs::write(&js, MAIN_JS).is_err() {
        return;
    }
    let Ok(sandbox) = aio_shell::runtime::sandbox_for(&rt, false) else { return };
    let mut cmd = Command::new(aio_shell::runtime::exe(&rt));
    put(&mut cmd);
    cmd.arg(&js)
        .args(sandbox)
        .arg(format!("--user-data-dir={}", dir.join("profile").display()))
        .arg("--ozone-platform-hint=auto")
        .arg(format!("--ax-title={title}"));
    start(cmd);
}

/// The splash process `cmd` started, told what ax says over its stdin.
fn start(mut cmd: Command) {
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else { return };
    let Some(stdin) = child.stdin.take() else { return };
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if line == "shown" {
                    eprintln!("ax dev: a splash shows the build's progress until the window opens");
                }
            }
        });
    }
    ENDED.store(false, std::sync::atomic::Ordering::Relaxed);
    BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
    with(|s| *s = Some(Open { child, stdin }));
    *crate::hotpatch::cargo_out::WATCHED.lock().unwrap_or_else(|e| e.into_inner()) = Some(progress);
    std::thread::spawn(|| {
        std::thread::sleep(SHOW_AFTER);
        while !ENDED.load(std::sync::atomic::Ordering::Relaxed) {
            if BUSY.load(std::sync::atomic::Ordering::Relaxed) {
                say(&aio::serde_json::json!({ "t": "show" }));
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    });
}

/// Tell the splash `m` (nothing when none is up).
pub fn say(m: &aio::serde_json::Value) {
    with(|s| {
        if let Some(o) = s.as_mut()
            && writeln!(o.stdin, "{m}").is_err()
        {
            *s = None;
        }
    });
}

/// cargo's count: `n` of `m` units, on `names`.
pub fn progress(n: u32, m: u32, names: &str) {
    if !names.is_empty() {
        busy();
    }
    say(&aio::serde_json::json!({ "t": "progress", "n": n, "m": m, "names": names }));
}

/// Work worth a splash has started (see [`BUSY`]).
pub fn busy() {
    BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The build failed with `errors`: shown (and kept up until a good build).
pub fn failed(errors: &str) {
    ENDED.store(true, std::sync::atomic::Ordering::Relaxed);
    say(&aio::serde_json::json!({ "t": "failed", "text": errors }));
}

/// The app was built but didn't start (`why`): said in place of the
/// window, kept up until a start that opens one.
pub fn not_started(why: &str) {
    ENDED.store(true, std::sync::atomic::Ordering::Relaxed);
    say(&aio::serde_json::json!({ "t": "failed", "start": true, "text": why }));
}

/// The build is done: the splash says so and waits for the app's window.
pub fn built() {
    ENDED.store(true, std::sync::atomic::Ordering::Relaxed);
    say(&aio::serde_json::json!({ "t": "built" }));
}

/// The splash goes (it asks its own process to end; killed if it doesn't).
pub fn close() {
    *crate::hotpatch::cargo_out::WATCHED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let Some(mut o) = with(Option::take) else { return };
    _ = writeln!(o.stdin, "{}", aio::serde_json::json!({ "t": "close" }));
    drop(o.stdin);
    std::thread::spawn(move || {
        for _ in 0..30 {
            if o.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        _ = o.child.kill();
        _ = o.child.wait();
    });
}

/// Once `up()` says the app's window is up (at most 30 s), the splash goes.
pub fn close_when_up(up: impl Fn() -> bool + Send + 'static) {
    if !active() {
        return;
    }
    std::thread::spawn(move || {
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < until && active() && !up() {
            std::thread::sleep(Duration::from_millis(100));
        }
        close();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargos_bar_is_its_count_and_crates() {
        let bar = "\r    Building [=====>                 ] 24/93: syn, tokio(build)   \r";
        use crate::hotpatch::cargo_out::{parse_progress, plain};
        assert_eq!(parse_progress(bar), Some((24, 93, "syn, tokio(build)".into())));
        let colored = "\x1b[1m\x1b[36m    Building\x1b[0m [==>   ] 3/10: a";
        assert_eq!(parse_progress(&plain(colored.as_bytes())), Some((3, 10, "a".into())));
        assert_eq!(parse_progress("    Building [=======] 10/10"), Some((10, 10, String::new())));
        assert_eq!(parse_progress("   Compiling a v0.1.0"), None);
    }

    /// What the splash was told, one JSON line each.
    #[cfg(unix)]
    fn told(file: &std::path::Path) -> Vec<String> {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        text.lines().map(|l| aio::serde_json::from_str::<aio::serde_json::Value>(l).unwrap()["t"].to_string()).collect()
    }

    #[cfg(unix)]
    #[test]
    fn a_splash_shows_for_a_build_with_work_and_goes_once_the_window_is_up() {
        let d = tempfile::tempdir().unwrap();
        let (quick, slow) = (d.path().join("quick"), d.path().join("slow"));
        let splash = |to: &std::path::Path| {
            let mut c = Command::new("sh");
            c.args(["-c", &format!("cat > {}", to.display())]);
            start(c);
        };
        // A build of nothing (the last fat binary): never shown.
        splash(&quick);
        assert!(active());
        std::thread::sleep(SHOW_AFTER * 3);
        built();
        close_when_up(|| true);
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while active() && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!active(), "gone once the window is up");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(told(&quick), ["\"built\"", "\"close\""]);
        // A build with crates to compile: shown, its count, then a launch that failed.
        splash(&slow);
        progress(3, 10, "syn");
        std::thread::sleep(SHOW_AFTER * 3);
        built();
        not_started("notes didn't start");
        close();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(told(&slow), ["\"progress\"", "\"show\"", "\"built\"", "\"failed\"", "\"close\""]);
    }

    #[test]
    fn the_title_cant_end_the_splash_pages_script() {
        assert!(MAIN_JS.contains("replace(/</g, '\\\\u003c')"), "JSON in <script> escapes <");
        assert!(!MAIN_JS.contains("${JSON.stringify(title)}"), "every title in the script goes through js()");
    }

    #[test]
    fn the_splash_page_takes_every_message_ax_sends() {
        for t in ["show", "progress", "built", "failed", "close"] {
            assert!(MAIN_JS.contains(&format!("'{t}'")), "{t}");
        }
        assert!(MAIN_JS.contains("sandbox: true") && MAIN_JS.contains("nodeIntegration: false"));
    }
}

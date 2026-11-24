//! A link to the app's scheme (`[app] schemes`) reaches it through one
//! door, `aio::ui::links`, whether it started the app or the app was
//! running: the later launch hands it over and ends. Real processes: this
//! test binary run again as the app, on a data dir of its own.
#![cfg(unix)]
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const ID: &str = "links-e2e";
const TEST: &str = "a_link_reaches_the_app_whether_it_started_it_or_it_was_running";

/// The app this binary is, run again ([`TEST`] with `LINKS_ROLE` set):
/// each link it hears goes on a line of `LINKS_OUT`.
fn app(role: &str) {
    let (mode, ui) = if role == "server" { ("server", "window") } else { ("client-server", "browser") };
    let toml = format!(
        "[app]\nid = \"{ID}\"\nmode = \"{mode}\"\nui = \"{ui}\"\nschemes = [\"lnk\"]\n[server]\nlisten = \"127.0.0.1:0\"\n"
    );
    let cfg = aio::config::Config::parse(&toml).unwrap();
    let aio_shell::Start::Run(dirs) = aio_shell::start(&cfg, "1.0.0").unwrap() else { return };
    let out = std::env::var_os("LINKS_OUT").unwrap();
    aio::rt().spawn(async move {
        let mut links = aio::ui::links();
        while let Some(l) = links.recv().await {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&out).unwrap();
            writeln!(f, "{l}").unwrap();
        }
    });
    if role == "server" {
        return aio_shell::serve(&dirs).unwrap();
    }
    // Shown in the browser: where it would serve (no server needed here).
    std::fs::write(dirs.data.join("serving.json"), r#"{"port":7420}"#).unwrap();
    fn view() -> aio::ui::View {
        aio::ui::prelude::rsx! { p { "app" } }
    }
    aio_shell::run(view, aio_shell::Window::default(), &dirs).unwrap();
}

fn launch(dir: &Path, role: &str, args: &[&str]) -> Command {
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args([TEST, "--exact", "--nocapture", "--test-threads=1"])
        .args(args)
        .env("LINKS_ROLE", role)
        .env("LINKS_OUT", dir.join("links.txt"))
        .env("AIORT_DATA", dir.join("app"))
        // A contained run: its browser page opens nowhere, and says so.
        .env("AIORT_NO_OPEN", "1")
        .env("HOME", dir.join("home"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY");
    c
}

fn heard(dir: &Path, n: usize) -> Vec<String> {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        let got: Vec<String> =
            std::fs::read_to_string(dir.join("links.txt")).unwrap_or_default().lines().map(String::from).collect();
        if got.len() >= n || Instant::now() > end {
            return got;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn stop(dir: &Path, mut first: Child) -> String {
    aio_shell::instance::quit_at(ID, &dir.join("app/data")).unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    while first.try_wait().unwrap().is_none() {
        assert!(Instant::now() < end, "the app did not stop");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::fs::read_to_string(dir.join("first.err")).unwrap()
}

#[test]
fn a_link_reaches_the_app_whether_it_started_it_or_it_was_running() {
    if let Ok(role) = std::env::var("LINKS_ROLE") {
        return app(&role);
    }
    for role in ["server", "browser"] {
        let d = tempfile::tempdir().unwrap();
        let err = std::fs::File::create(d.path().join("first.err")).unwrap();
        // Started by a link.
        let first =
            launch(d.path(), role, &["lnk://cold/1?q=a%20b"]).stderr(err).stdout(Stdio::null()).spawn().unwrap();
        let said = || std::fs::read_to_string(d.path().join("first.err")).unwrap();
        assert_eq!(heard(d.path(), 1), ["lnk://cold/1?q=a%20b"], "{role}: the cold start's link: {}", said());
        // A link while it runs, its scheme in capitals as an OS may hand it,
        // with a file and text after `--` that would be flags before it.
        let args = ["--", "LNK://warm/2", "notes.md", "--quit", "--takeover", "other://x"];
        let second = launch(d.path(), role, &args).output().unwrap();
        let said = String::from_utf8_lossy(&second.stderr);
        assert!(second.status.success(), "{role}: {said}");
        assert_eq!(heard(d.path(), 2), ["lnk://cold/1?q=a%20b", "LNK://warm/2"], "{role}: its own scheme's only");
        if role == "server" {
            assert!(said.contains(&format!("[aio] {ID} is already running, serving")), "{said}");
        }
        let first = stop(d.path(), first);
        assert!(first.contains("[aio] handed a launch"), "{role}: never a --quit or --takeover: {first}");
        if role == "browser" {
            // The page is opened again for the later launch (here: refused, said).
            let opened = first.matches("open http://127.0.0.1:7420/ in a browser (not opened (AIORT_NO_OPEN").count();
            assert_eq!(opened, 2, "{first}");
        }
    }
}

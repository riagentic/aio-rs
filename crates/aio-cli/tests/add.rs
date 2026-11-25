//! `ax add` (aio's `am add`): a cell and a module of server functions
//! added to a new app build, wired, and pass their tests as the app's own;
//! the first page crate moves the cells to the app's core crate (asked
//! first), each page reads them there, and it all builds, formatted.
//! Unix: ci-windows.sh runs tests cross-built, without the binary beside them.
#![cfg(unix)]
use std::path::Path;
use std::process::Command;

fn ok(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    let said = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{cmd:?}:\n{said}");
    said
}

#[test]
fn an_added_cell_and_server_functions_build_and_pass_their_tests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap();
    // Beside the checkout, as `--aio` needs (scripts/parity's folder too).
    std::fs::create_dir_all(root.join("target")).unwrap();
    let d = tempfile::Builder::new().prefix("aio-add-").tempdir_in(root.join("target")).unwrap();
    let target = std::env::var_os("AIO_PARITY_TARGET").map_or_else(|| root.join("target").join("parity"), Into::into);
    let run = |dir: &Path, args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ax"));
        c.args(args).current_dir(dir).env("AIORT_NO_WARM", "1").env_remove("RUSTUP_TOOLCHAIN").env_remove("CARGO");
        // Its check of the core crate builds where the test does, offline.
        c.env("CARGO_TARGET_DIR", &target).env("CARGO_NET_OFFLINE", "true").stdin(std::process::Stdio::null());
        c.output().unwrap()
    };
    let ax = |dir: &Path, args: &[&str]| {
        let out = run(dir, args);
        let said = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{args:?}:\n{said}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    ax(d.path(), &["create", "added", "--aio", root.to_str().unwrap(), "--no-warm"]);
    let app = d.path().join("added");
    std::fs::copy(root.join("Cargo.lock"), app.join("Cargo.lock")).unwrap();
    // Inside the app its name may be left out (aio's form), or given; a
    // pipe gets JSON (aio's am), --text the text.
    let said = ax(&app, &["add", "cell", "tally"]);
    assert!(said.contains(r#"{"ok":true,"value":{"changed":["src/lib.rs","app/src/main.rs""#), "{said}");
    let said = ax(&app, &["add", "added", "server", "tools", "--text"]);
    assert!(said.starts_with("created src/tools.rs\n"), "{said}");
    // The first page moves the cells to a core crate: asked first, so with
    // nobody to ask (and as JSON, the plan) nothing changes without --yes.
    let lib = app.join("src/lib.rs");
    let before = std::fs::read_to_string(&lib).unwrap();
    let out = run(&app, &["add", "added", "page", "start", "--text"]);
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("again with --yes"), "{out:?}");
    let said = ax(&app, &["add", "added", "page", "start", "--json"]);
    assert!(said.contains(r#""done":false"#) && said.contains(r#""package":"added-core""#), "{said}");
    assert!(said.contains(r#""removes":["src/tally.rs","src/tools.rs"]"#), "{said}");
    assert_eq!(std::fs::read_to_string(&lib).unwrap(), before);
    assert!(!app.join("pages").exists() && !app.join("core").exists());
    // What doesn't build so split (the view reads a field only the cell's
    // crate may) is refused whole: every file as it was.
    std::fs::write(&lib, before.replace("pub count: i64", "count: i64")).unwrap();
    let manifest = std::fs::read_to_string(app.join("Cargo.toml")).unwrap();
    let out = run(&app, &["add", "added", "page", "start", "--yes", "--text"]);
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && said.contains("nothing changed") && said.contains("private"), "{said}");
    assert_eq!(std::fs::read_to_string(app.join("Cargo.toml")).unwrap(), manifest);
    assert!(app.join("src/tally.rs").exists() && !app.join("pages").exists() && !app.join("core").exists());
    let copies = std::fs::read_dir(d.path()).unwrap().count();
    assert_eq!(copies, 1, "its copy removed");
    std::fs::write(&lib, &before).unwrap();
    let said = ax(&app, &["add", "added", "page", "start", "--yes", "--text"]);
    assert!(
        said.starts_with("created pages/start/src/lib.rs\nchanged Cargo.toml\nmade core/ (added-core): "),
        "{said}"
    );
    assert!(said.contains("\nmoved src/tally.rs to core/src/tally.rs\n"), "{said}");
    let get = |f: &str| std::fs::read_to_string(app.join(f)).unwrap();
    assert!(get("src/lib.rs").contains("\npub use added_core::*;\n") && !app.join("src/tally.rs").exists());
    assert!(get("core/src/lib.rs").contains("aio::client!(Counter, counter);"));
    assert!(!get("core/src/lib.rs").contains("use aio::ui::prelude::*;"), "unused there: dropped");
    assert!(get("pages/start/src/lib.rs").contains("use added_core::prelude::*;"));
    // Then a page is a page on it, and a cell goes to it.
    let said = ax(&app, &["add", "page", "more", "--text"]);
    assert!(said.starts_with("created pages/more/src/lib.rs\nchanged Cargo.toml\nshow it"), "{said}");
    assert!(get("pages/more/Cargo.toml").contains("added-core = { path = \"../../core\" }"));
    let said = ax(&app, &["add", "cell", "extra", "--text"]);
    assert!(said.starts_with("created core/src/extra.rs\nchanged core/src/lib.rs\nchanged src/lib.rs\n"), "{said}");
    let said = ax(&app, &["add", "server", "lookup", "--text"]);
    assert!(said.starts_with("created core/src/lookup.rs\nchanged core/src/lib.rs\nchanged app/src/main.rs"), "{said}");
    // A page's view runs the app's server functions (a client's copy runs them on its server).
    let more = get("pages/more/src/lib.rs");
    let run = "\n/// The server's tools, as a client's view asks for them; a local cell has no server.\n\
               #[cfg(not(target_arch = \"wasm32\"))]\npub async fn tools() -> Option<Vec<String>> {\n    \
               let aio::Source::Remote(r) = HANDLE.read_untracked().clone() else { return None };\n    \
               r.run(added_core::tools::LIST, ()).await.ok()\n}\n";
    std::fs::write(app.join("pages/more/src/lib.rs"), more + run).unwrap();
    // The app's view shows both (each reading the cell).
    let main = "        main { class: \"container\",\n";
    let text = get("src/lib.rs");
    assert!(text.contains(main), "{text}");
    let shown = format!("{main}            start::Start {{}}\n            more::More {{}}\n");
    let one = "        assert_eq!(ui.text(\"count\"), \"1\");\n";
    assert!(text.contains(one), "{text}");
    let read = format!("{one}        assert!(ui.html().contains(\"<p>count: 1</p>\"), \"the pages read the cell\");\n");
    std::fs::write(&lib, text.replacen(main, &shown, 1).replacen(one, &read, 1)).unwrap();
    // As `ax check` has it: formatted as cargo fmt writes it.
    let mut fmt = Command::new("cargo");
    ok(fmt.args(["fmt", "--all", "--check"]).current_dir(&app).env_remove("RUSTUP_TOOLCHAIN").env_remove("CARGO"));
    let mut test = Command::new("cargo");
    // Every crate built (the launcher's wiring too); the added parts' tests run.
    test.args([
        "test",
        "--offline",
        "--workspace",
        "--",
        "tally::",
        "tools::",
        "lookup::",
        "extra::",
        "tests::the_buttons",
    ])
    .current_dir(&app)
    .env("CARGO_TARGET_DIR", target)
    .env_remove("RUSTUP_TOOLCHAIN")
    .env_remove("CARGO");
    let said = ok(&mut test);
    for t in [
        "tally::tests::add_counts ... ok",
        "extra::tests::add_counts ... ok",
        "tools::tests::list_answers ... ok",
        "lookup::tests::list_answers ... ok",
        "tests::the_buttons_count_and_reset ... ok",
    ] {
        assert!(said.contains(t), "{t}:\n{said}");
    }
}

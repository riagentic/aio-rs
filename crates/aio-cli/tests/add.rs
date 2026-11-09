//! `ax add` (aio's `am add`): a cell and a module of server functions
//! added to a new app build, wired, and pass their tests as the app's own;
//! a page crate added builds, its view shown by the app's.
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
    let ax = |dir: &Path, args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ax"));
        c.args(args).current_dir(dir).env("AIORT_NO_WARM", "1").env_remove("RUSTUP_TOOLCHAIN").env_remove("CARGO");
        ok(&mut c)
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
    let said = ax(&app, &["add", "added", "page", "start", "--text"]);
    assert!(said.starts_with("created pages/start/src/lib.rs\nchanged Cargo.toml\n"), "{said}");
    let lib = app.join("src/lib.rs");
    let text = std::fs::read_to_string(&lib).unwrap();
    std::fs::write(&lib, text.replacen("        main {\n", "        main {\n            start::Start {}\n", 1))
        .unwrap();
    let target = std::env::var_os("AIO_PARITY_TARGET").map_or_else(|| root.join("target").join("parity"), Into::into);
    let mut test = Command::new("cargo");
    // Every crate built (the launcher's wiring too); the added parts' tests run.
    test.args(["test", "--offline", "--workspace", "--", "tally::", "tools::", "tests::the_buttons"])
        .current_dir(&app)
        .env("CARGO_TARGET_DIR", target)
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("CARGO");
    let said = ok(&mut test);
    for t in [
        "tally::tests::add_counts ... ok",
        "tools::tests::list_answers ... ok",
        "tests::the_buttons_count_and_reset ... ok",
    ] {
        assert!(said.contains(t), "{t}:\n{said}");
    }
}

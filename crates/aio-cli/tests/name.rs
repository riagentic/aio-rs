//! The command is `ax` (aio's is `am`): the binary has that name, and what it
//! prints never sends anyone to `aio <verb>`.
use std::process::Command;

fn ax(args: &[&str]) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_ax")).args(args).output().unwrap();
    assert!(o.status.success(), "ax {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Each `aio` followed by a word: what reads as the old command.
fn old_command(text: &str) -> Vec<String> {
    let b = text.as_bytes();
    let mut found = vec![];
    for (i, _) in text.match_indices("aio ") {
        let before = i.checked_sub(1).map(|j| b[j]);
        let word: String = text[i + 4..].chars().take_while(|c| c.is_ascii_lowercase() || *c == '-').collect();
        if !before.is_some_and(|c| c.is_ascii_alphanumeric() || b"_-./:".contains(&c)) && !word.is_empty() {
            found.push(format!("aio {word}"));
        }
    }
    found
}

#[test]
// ci-windows.sh runs tests cross-built here, without the binary beside them.
#[cfg_attr(windows, ignore = "needs the ax binary at its build path")]
fn the_binary_is_ax_and_its_help_names_only_ax() {
    let exe = std::path::Path::new(env!("CARGO_BIN_EXE_ax"));
    assert_eq!(exe.file_stem().and_then(|s| s.to_str()), Some("ax"));
    assert!(ax(&["--version"]).starts_with("aio-rs "));
    let help = ax(&["help"]);
    // Each usage line's verb: its words up to the first argument (`snapshot save <app>`).
    let verbs: Vec<Vec<&str>> = help
        .lines()
        .filter_map(|l| l.strip_prefix("  ax ")?.split("  ").next())
        .map(|u| u.split(' ').take_while(|w| !w.starts_with(['<', '['])).collect())
        .collect();
    assert!(verbs.contains(&vec!["dev"]) && verbs.contains(&vec!["build"]), "{help}");
    let mut all = vec![help.clone(), ax(&["agent", "--max"])];
    all.extend(verbs.iter().map(|v| ax(&[&["help"], &v[..]].concat())));
    for text in &all {
        assert_eq!(old_command(text), Vec::<String>::new(), "{text}");
    }
    assert_eq!(old_command("run aio dev, then aio build"), ["aio dev", "aio build"]);
    assert!(old_command("aio-rs 0.1.0, aio.toml, use aio::prelude").is_empty());
}

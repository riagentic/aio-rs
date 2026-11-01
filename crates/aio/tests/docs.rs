//! The guide's `aio.toml` examples (`docs/`): every TOML block that is a
//! whole file (it has `[app]`) parses as an app's config does. Its Rust
//! examples are doctests (`src/guide.rs`).
use std::path::{Path, PathBuf};

/// Pages of measurements, not the guide.
const RECORDS: [&str; 1] = ["comparison.md"];

fn pages(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            pages(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

/// The ```toml blocks of a page.
fn toml_blocks(text: &str) -> Vec<String> {
    let (mut blocks, mut open) = (vec![], None::<String>);
    for line in text.lines() {
        match (&mut open, line.trim_start()) {
            (None, l) if l.starts_with("```toml") => open = Some(String::new()),
            (Some(_), l) if l.starts_with("```") => blocks.extend(open.take()),
            (Some(b), _) => {
                b.push_str(line);
                b.push('\n');
            }
            _ => {}
        }
    }
    blocks
}

#[test]
fn every_aio_toml_in_the_guide_parses() {
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    if !docs.is_dir() {
        // A test binary run away from its checkout (ci-windows.sh copies only the .exe).
        eprintln!("skipped: no {}", docs.display());
        return;
    }
    let mut all = vec![];
    pages(&docs, &mut all);
    all.retain(|p| !(p.parent() == Some(&*docs) && RECORDS.iter().any(|r| p.ends_with(r))));
    let mut checked = 0;
    for page in all {
        for b in toml_blocks(&std::fs::read_to_string(&page).unwrap()).iter().filter(|b| b.contains("[app]")) {
            if let Err(e) = aio::config::Config::parse(b) {
                panic!("{}: an aio.toml example doesn't parse: {e}\n{b}", page.display());
            }
            checked += 1;
        }
    }
    assert!(checked >= 2, "the guide's aio.toml examples went missing ({checked})");
}

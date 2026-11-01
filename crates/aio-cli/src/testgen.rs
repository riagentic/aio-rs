//! `ax testgen <app>` (aio's `am testgen`): a typed name table of what the
//! running app's window shows, for its UI tests. Each `data-t` handle and
//! each auto name (TestUi's: `SaveButton`, `EmailInput`) is a `&str` const,
//! and each handle a module of the names inside it (`row_1::DeleteButton`
//! is `"row-1/DeleteButton"`), so a test says `ui.click(ui_names::SaveButton)`
//! and an element renamed in the app is a compile error at the next
//! `ax testgen`, not a runtime miss.
use crate::cdp::Session;
use std::collections::BTreeMap;
use std::path::Path;

/// Where the table goes unless `--out` says: a module the app's tests
/// take with `mod ui_names;` (a folder's `mod.rs` is no test of its own).
pub const OUT: &str = "tests/ui_names/mod.rs";

/// `name` as a Rust identifier (pure): `row-1` is `row_1`, a leading digit
/// gets `_`, a keyword `r#`.
pub fn ident(name: &str) -> String {
    let mut s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    const KEYWORDS: [&str; 38] = [
        "as", "async", "await", "break", "const", "continue", "dyn", "else", "enum", "extern", "false", "fn", "for",
        "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "static", "struct",
        "trait", "true", "type", "unsafe", "use", "where", "while", "abstract", "become", "box", "gen",
    ];
    if KEYWORDS.contains(&s.as_str()) { format!("r#{s}") } else { s }
}

/// The table's Rust (pure): `names` (top level), `handles`, each with the
/// names inside it. Two names that make one identifier keep the first.
pub fn table(app: &str, names: &[String], handles: &BTreeMap<String, Vec<String>>) -> String {
    let consts = |out: &mut String, pad: &str, items: &mut dyn Iterator<Item = (String, String)>| {
        let mut seen = std::collections::BTreeSet::new();
        for (id, value) in items {
            if seen.insert(id.clone()) {
                out.push_str(&format!("{pad}pub const {id}: &str = {value:?};\n"));
            }
        }
    };
    let mut out = format!(
        "//! Written by `ax testgen {app}` from what its window showed: each handle\n\
         //! and auto name a const, so a renamed element fails to compile. Run it\n\
         //! again after a UI change; edits here are lost then.\n\
         #![allow(dead_code, non_upper_case_globals, non_snake_case)]\n\n"
    );
    let top = handles.keys().chain(names).map(|n| (ident(n), n.clone()));
    consts(&mut out, "", &mut top.collect::<Vec<_>>().into_iter());
    for (h, inner) in handles.iter().filter(|(_, inner)| !inner.is_empty()) {
        out.push_str(&format!("\n/// Inside `{h}`.\npub mod {} {{\n", ident(h)));
        consts(&mut out, "    ", &mut inner.iter().map(|n| (ident(n), format!("{h}/{n}"))));
        out.push_str("}\n");
    }
    out
}

/// The handles in the window, each with the auto names inside it.
async fn handles(s: &mut Session) -> Result<BTreeMap<String, Vec<String>>, String> {
    let ask = |op: &str, arg: aio::serde_json::Value| {
        format!("({})({}, {arg})", aio::ui::SURFACE_JS, aio::serde_json::json!(op))
    };
    let list = |v: &aio::serde_json::Value| -> Vec<String> {
        v["ok"].as_array().into_iter().flatten().filter_map(|n| n.as_str().map(String::from)).collect()
    };
    let mut out = BTreeMap::new();
    for h in list(&s.eval(&ask("handles", aio::serde_json::json!({}))).await?) {
        let inner = list(&s.eval(&ask("names", aio::serde_json::json!({ "path": h }))).await?);
        out.insert(h, inner);
    }
    Ok(out)
}

/// Write the table of the window `s` of app `app` to `out`.
pub async fn run(s: &mut Session, app: &str, out: &Path) -> Result<String, String> {
    let names = crate::surface::names(s).await?;
    let handles = handles(s).await?;
    if names.is_empty() && handles.is_empty() {
        return Err(format!("{app}'s window shows nothing to name (is it on its first screen yet? --wait-client)"));
    }
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(out, table(app, &names, &handles)).map_err(|e| format!("{}: {e}", out.display()))?;
    let use_it = match out.strip_prefix("tests").ok().and_then(|p| p.iter().next()) {
        Some(m) => format!("`mod {};` in a test of tests/", Path::new(m).with_extension("").display()),
        None => "include it in a test".into(),
    };
    Ok(format!(
        "wrote {}: {} handles, {} auto names; {use_it}, then ui.click(ui_names::…)",
        out.display(),
        handles.len(),
        names.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_an_identifier() {
        assert_eq!(ident("row-1"), "row_1");
        assert_eq!(ident("SaveButton"), "SaveButton");
        assert_eq!(ident("2fa"), "_2fa");
        assert_eq!(ident("type"), "r#type");
        assert_eq!(ident(""), "_");
    }

    #[test]
    fn the_table_is_a_const_a_name_and_a_module_a_handle() {
        let handles =
            BTreeMap::from([("row-1".to_string(), vec!["DeleteButton".to_string()]), ("agree".into(), vec![])]);
        let t = table("notes", &["SaveButton".into(), "SaveButton2".into(), "row_1".into()], &handles);
        assert!(t.starts_with("//! Written by `ax testgen notes`"), "{t}");
        assert!(t.contains("pub const agree: &str = \"agree\";\npub const row_1: &str = \"row-1\";\n"), "{t}");
        assert!(t.contains("pub const SaveButton2: &str = \"SaveButton2\";\n"), "{t}");
        assert_eq!(t.matches("pub const row_1").count(), 1, "the first of one identifier: {t}");
        assert!(t.contains("pub mod row_1 {\n    pub const DeleteButton: &str = \"row-1/DeleteButton\";\n}\n"), "{t}");
        assert!(!t.contains("pub mod agree"), "nothing inside: no module");
    }

    /// The table of a real page (skipped without Chromium) names what
    /// TestUi finds there.
    #[cfg(unix)]
    #[test]
    fn the_table_names_the_window_as_testui_does() {
        let html = "<button>Save</button><ul><li data-t=row-1>One <button>Delete</button></li></ul>";
        let Some(page) = crate::cdp::tests::Chromium::open(html) else { return };
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("tests/ui_names/mod.rs");
        aio::rt().block_on(async {
            let mut s = page.session().await;
            let said = run(&mut s, "notes", &out).await.unwrap();
            assert!(said.contains("1 handles"), "{said}");
        });
        let t = std::fs::read_to_string(&out).unwrap();
        assert!(t.contains("pub const SaveButton: &str = \"SaveButton\";"), "{t}");
        assert!(t.contains("pub const DeleteButton: &str = \"row-1/DeleteButton\";"), "{t}");
    }
}

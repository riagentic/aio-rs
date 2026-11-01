//! A store's states replayed by the app's own code, for an app that isn't
//! running or a store that isn't its (`ax timeline --from`, `ax record` on a
//! stopped app; aio's journal of a stopped app). The running app answers
//! `at` on its door; with none, a test of the app's written for the run
//! (`ax preview`'s way) opens each of its cells (every `#[cell]` struct in
//! its library) on a copy of the store and computes the state right after
//! each event asked for, as the app would: its methods replayed from the
//! nearest snapshot. Nothing in the store moves: the copy goes after it.
use aio::serde_json::{self, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The test the states are computed by, `tests/aio_states.rs` in the app.
const NAME: &str = "aio_states";
const BEGIN: &str = "--- ax states ---";
const END: &str = "--- ax states end ---";

/// The paths in the crate of the `#[cell]` structs of `files` ((path under
/// `src/`, its Rust)): `Notes`, `board::Board`. A file that doesn't parse is
/// skipped (the build will say why).
pub fn cell_types(files: &[(PathBuf, String)]) -> Vec<String> {
    fn walk(items: &[syn::Item], at: &str, out: &mut Vec<String>) {
        for item in items {
            match item {
                syn::Item::Struct(s)
                    if s.attrs.iter().any(|a| a.path().segments.last().is_some_and(|p| p.ident == "cell")) =>
                {
                    let name = s.ident.to_string();
                    out.push(if at.is_empty() { name } else { format!("{at}::{name}") });
                }
                syn::Item::Mod(m) if let Some((_, inner)) = &m.content => {
                    let name = m.ident.to_string();
                    walk(inner, &if at.is_empty() { name } else { format!("{at}::{name}") }, out);
                }
                _ => {}
            }
        }
    }
    let mut out = vec![];
    for (path, text) in files {
        let Ok(file) = syn::parse_file(text) else { continue };
        let mut parts: Vec<String> = path.with_extension("").iter().map(|p| p.to_string_lossy().into_owned()).collect();
        if matches!(parts.last().map(String::as_str), Some("lib" | "mod" | "main")) {
            parts.pop();
        }
        walk(&file.items, &parts.join("::"), &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// The test that computes the states of `types` (cells of crate `krate`)
/// (pure). The store and what to compute come from `AIO_STATES_*`, which
/// [`states`] sets.
pub fn test(krate: &str, types: &[String]) -> String {
    let each: String = types.iter().map(|t| format!("    at::<{krate}::{t}>(store, &ask, &mut out);\n")).collect();
    format!(
        "//! Written by `ax timeline` and `ax record` for one run, then removed:\n\
         //! the app's cells replayed from a copy of its store, by its own code.\n\
         #[test]\n\
         fn aio_states() {{\n    \
         // Set by ax; without them there is nothing to replay.\n    \
         let (Ok(store), Ok(ask)) = (std::env::var(\"AIO_STATES_STORE\"), std::env::var(\"AIO_STATES_ASK\")) else {{\n        \
         return;\n    \
         }};\n    \
         println!(\"\\n{BEGIN}\\n{{}}\\n{END}\", states(&store, &ask));\n\
         }}\n\n\
         type Map = aio::serde_json::Map<String, aio::serde_json::Value>;\n\n\
         /// Cell `T`'s whole state right after each event `ask` names for it.\n\
         fn at<T: aio::Cell>(store: &str, ask: &aio::serde_json::Value, out: &mut Map) {{\n    \
         let Some(seqs) = ask[T::ID].as_array() else {{ return }};\n    \
         let s = match aio::Store::<T>::open(store) {{\n        \
         Ok(s) => s,\n        \
         Err(e) => {{\n            \
         out.insert(T::ID.into(), aio::serde_json::json!({{ \"error\": e.0 }}));\n            \
         return;\n        \
         }}\n    \
         }};\n    \
         let mut m = Map::new();\n    \
         for q in seqs.iter().filter_map(|q| q.as_u64()) {{\n        \
         let v = match s.state_at(q) {{\n            \
         Ok(st) => aio::serde_json::to_value(&st).unwrap_or_default(),\n            \
         Err(e) => aio::serde_json::json!({{ \"error\": e.0 }}),\n        \
         }};\n        \
         m.insert(q.to_string(), v);\n    \
         }}\n    \
         out.insert(T::ID.into(), m.into());\n\
         }}\n\n\
         /// Each cell's states `ask` names (`{{\"cell\": [seq, …]}}`), as JSON.\n\
         pub fn states(store: &str, ask: &str) -> String {{\n    \
         let ask: aio::serde_json::Value = aio::serde_json::from_str(ask).unwrap_or_default();\n    \
         let mut out = Map::new();\n\
         {each}    \
         aio::serde_json::Value::Object(out).to_string()\n\
         }}\n"
    )
}

/// Every `.rs` file under `src`, as (its path under `src`, its text).
fn sources(src: &Path) -> Vec<(PathBuf, String)> {
    let mut out = vec![];
    let mut dirs = vec![src.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "rs")
                && let (Ok(rel), Ok(text)) = (p.strip_prefix(src), std::fs::read_to_string(&p))
            {
                out.push((rel.to_path_buf(), text));
            }
        }
    }
    out
}

/// What [`states`] answers: by cell, the state right after each event.
pub type States = BTreeMap<String, BTreeMap<u64, Value>>;

/// The answer of the written test (pure): a cell it couldn't open, or an
/// event it couldn't replay, is an error naming it.
pub fn read(json: &str) -> Result<States, String> {
    let all: serde_json::Map<String, Value> =
        serde_json::from_str(json).map_err(|e| format!("the replay said: {e}"))?;
    let mut out = States::new();
    for (cell, v) in all {
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            return Err(format!("{cell}: {e}"));
        }
        let mut at = BTreeMap::new();
        for (seq, st) in v.as_object().into_iter().flatten() {
            if let Some(e) =
                st.get("error").and_then(Value::as_str).filter(|_| st.as_object().is_some_and(|o| o.len() == 1))
            {
                return Err(format!("{cell} at event {seq}: {e}"));
            }
            at.insert(seq.parse().map_err(|_| format!("{cell}: {seq} is no event"))?, st.clone());
        }
        out.insert(cell, at);
    }
    Ok(out)
}

/// The states `ask` names (by cell, events; each with the one before it,
/// for its diff) of the store `store`, replayed by the app library `pkg`
/// (in `dir`) on a copy, built by cargo as its tests are.
pub fn states(pkg: &str, dir: &Path, store: &Path, ask: &BTreeMap<String, Vec<u64>>) -> Result<States, String> {
    let types = cell_types(&sources(&dir.join("src")));
    if types.is_empty() {
        return Err(format!("{pkg}: no #[cell] struct in its src/ to replay the store with"));
    }
    let ask: BTreeMap<&String, Vec<u64>> = ask
        .iter()
        .map(|(c, seqs)| {
            let mut all: Vec<u64> = seqs.iter().flat_map(|s| [s.saturating_sub(1), *s]).collect();
            all.sort_unstable();
            all.dedup();
            (c, all)
        })
        .collect();
    // A copy, consistent while the app may write: SQLite's own; gone after.
    struct Gone(PathBuf);
    impl Drop for Gone {
        fn drop(&mut self) {
            _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let tmp = Gone(std::env::temp_dir().join(format!("ax-states-{}-{}", std::process::id(), crate::now_ms())));
    aio::paths::private_dir(&tmp.0).map_err(|e| format!("{}: {e}", tmp.0.display()))?;
    let copy = tmp.0.join("state.db");
    crate::open(store)?
        .execute("VACUUM INTO ?1", [copy.to_string_lossy()])
        .map_err(|e| format!("{}: a copy to replay: {e}", store.display()))?;
    let (copy, ask) = (copy.to_string_lossy().into_owned(), serde_json::to_string(&ask).map_err(|e| e.to_string())?);
    let env = [("AIO_STATES_STORE", copy.as_str()), ("AIO_STATES_ASK", ask.as_str())];
    let out = crate::preview::run_written(pkg, dir, NAME, &test(&pkg.replace('-', "_"), &types), &env)
        .map_err(|e| format!("the app's code didn't replay its store:\n{e}"))?;
    read(&crate::preview::between(&out, BEGIN, END).ok_or_else(|| format!("the replay said nothing:\n{out}"))?)
}

/// The states written by `tests::the_written_replay_computes_the_states`, as a
/// test of ours: it compiles, and it replays.
#[cfg(test)]
#[path = "replayed_fixture.rs"]
#[rustfmt::skip]
mod written;

#[cfg(test)]
mod tests {
    use super::*;
    use aio::prelude::*;

    #[cell(persist)]
    pub struct Tally {
        pub n: u32,
    }

    #[methods]
    impl Tally {
        pub fn add(&mut self, by: u32) {
            self.n += by;
        }
    }

    #[test]
    fn every_cell_struct_is_found() {
        let f = |p: &str, t: &str| (PathBuf::from(p), t.to_string());
        let files = [
            f("lib.rs", "#[aio::cell(persist)]\npub struct Notes { pub t: String }\nmod board;"),
            f("board/mod.rs", "pub mod deep { #[cell] pub struct Board {} }\npub struct Plain;"),
            f("bad.rs", "fn ("),
        ];
        assert_eq!(cell_types(&files), ["Notes", "board::deep::Board"]);
        let t = test("notes", &cell_types(&files));
        assert!(
            t.contains("at::<notes::Notes>(store, &ask, &mut out);") && t.contains("at::<notes::board::deep::Board>")
        );
    }

    #[test]
    fn the_written_replay_computes_the_states() {
        assert_eq!(test("crate", &["replayed::tests::Tally".into()]), include_str!("replayed_fixture.rs"));
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state.db");
        let h = Handle::<Tally>::open(&p).unwrap();
        for by in [1, 2, 3] {
            h.add(by).unwrap();
        }
        drop(h);
        let said = written::states(&p.to_string_lossy(), r#"{ "tally": [0, 2, 3, 9], "other": [1] }"#);
        let v: Value = serde_json::from_str(&said).unwrap();
        assert_eq!(
            (&v["tally"]["0"]["n"], &v["tally"]["2"]["n"], &v["tally"]["3"]["n"]),
            (&serde_json::json!(0), &serde_json::json!(3), &serde_json::json!(6))
        );
        assert!(v["tally"]["9"]["error"].as_str().unwrap().contains("no event 9"), "{v}");
        assert!(v.get("other").is_none(), "a cell the app hasn't: not asked of it");
        assert!(read(&said).unwrap_err().contains("tally at event 9: "), "an event it can't replay is said");
        let ok = read(r#"{ "tally": { "1": { "n": 1 }, "2": { "error": "x", "n": 2 } } }"#).unwrap();
        assert_eq!(ok["tally"][&2]["n"], 2, "a state with a field named error is a state");
    }

    #[test]
    fn its_output_is_told_from_the_tests() {
        let out = format!("running 1 test\n\n{BEGIN}\n{{}}\n{END}\ntest aio_states ... ok\n");
        assert_eq!(crate::preview::between(&out, BEGIN, END).unwrap(), "{}");
    }
}

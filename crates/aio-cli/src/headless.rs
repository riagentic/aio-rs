//! `ax surface <app> --headless` (aio's server-side surface): the app's
//! views read with no window open (a server, a closed window). The design:
//! the views are mounted where its tests mount them, in the hidden page of
//! `aio::testing::TestUi`, by a test of the app's written for the run
//! (`ax preview`'s way), and each of its cells (every `aio::client!` in
//! its library, so a multi-cell app shows whole) is the running app's live
//! cell: a synced copy over the app's local door, as the user running the
//! command. So the views render the state the app has now, as a client
//! sees it, and nothing in the app's binary grows (it holds no headless
//! DOM); the views are the source's as built now, which the test compiles.
use std::path::{Path, PathBuf};

/// The test a headless surface runs as, `tests/aio_surface.rs` in the app.
const NAME: &str = "aio_surface";
const BEGIN: &str = "--- ax surface ---";
const END: &str = "--- ax surface end ---";

/// The modules of `files` ((path under `src/`, its Rust)) that hold an
/// `aio::client!`, as paths in the crate (`""`: its root; `notes::list`).
/// A file that doesn't parse is skipped (the build will say why).
pub fn client_modules(files: &[(PathBuf, String)]) -> Vec<String> {
    fn walk(items: &[syn::Item], at: &str, out: &mut Vec<String>) {
        for item in items {
            match item {
                syn::Item::Macro(m) if m.mac.path.segments.last().is_some_and(|s| s.ident == "client") => {
                    out.push(at.to_string())
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

/// The test that mounts `root` (a component of crate `krate`) with each of
/// `modules`' cells live (pure). The app, its data dir and the user come
/// from `AIO_SURFACE_*`, which `ax surface --headless` sets.
pub fn test(krate: &str, root: &str, modules: &[String]) -> String {
    let at = |m: &String| if m.is_empty() { krate.to_string() } else { format!("{krate}::{m}") };
    let (mut open, mut set) = (String::new(), String::new());
    for (i, m) in modules.iter().enumerate() {
        open.push_str(&format!("    let c{i} = live(&{}::HANDLE, app, data, user);\n", at(m)));
        set.push_str(&format!("            *{}::HANDLE.write() = c{i};\n", at(m)));
    }
    format!(
        "//! Written by `ax surface --headless` for one run, then removed: the\n\
         //! app's views, on its running cells.\n\
         #[test]\n\
         fn aio_surface() {{\n    \
         // Set by `ax surface --headless`; without them there is nothing to show.\n    \
         let (Ok(app), Ok(data), Ok(user)) = (\n        \
         std::env::var(\"AIO_SURFACE_APP\"),\n        \
         std::env::var(\"AIO_SURFACE_DATA\"),\n        \
         std::env::var(\"AIO_SURFACE_USER\"),\n    \
         ) else {{\n        \
         return;\n    \
         }};\n    \
         println!(\"\\n{BEGIN}\\n{{}}\\n{END}\", surface(&app, &data, &user));\n\
         }}\n\n\
         /// Cell `T`'s live copy, over the running app's local door.\n\
         fn live<T: aio::Cell>(\n    \
         _: &aio::ui::Global<aio::Source<T>>,\n    \
         app: &str,\n    \
         data: &str,\n    \
         user: &str,\n\
         ) -> aio::Source<T> {{\n    \
         let door = aio::sync::door(app, std::path::Path::new(data), T::ID).expect(\"the app's door\");\n    \
         aio::Source::connect(&door, \"\", user).unwrap_or_else(|e| panic!(\"{{}}: {{e}}\", T::ID))\n\
         }}\n\n\
         /// `{root}` on the live cells, as a person meets it.\n\
         pub fn surface(app: &str, data: &str, user: &str) -> String {{\n\
         {open}    \
         let ui = aio::testing::Mount::default()\n        \
         .before(move || {{\n\
         {set}        \
         }})\n        \
         .app({krate}::{root});\n    \
         ui.surface()\n\
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

/// The headless surface of the running app `id` (its data in `data`):
/// `root` of its library `pkg` (in `dir`) mounted on its live cells, as
/// `user`.
pub fn run(pkg: &str, dir: &Path, root: &str, id: &str, data: &Path, user: &str) -> Result<String, String> {
    if root.is_empty() || !root.split("::").all(|p| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '_'))
    {
        return Err(format!("--root {root}: a component's path in the app's crate (App, ui::Shell)"));
    }
    if !aio::paths::running(data) {
        return Err(format!("{id} isn't running: a headless surface reads its live cells"));
    }
    let modules = client_modules(&sources(&dir.join("src")));
    if modules.is_empty() {
        return Err(format!("{pkg}: no aio::client! in its src/, so no views read its cells"));
    }
    let data = data.to_string_lossy();
    let env = [("AIO_SURFACE_APP", id), ("AIO_SURFACE_DATA", &data), ("AIO_SURFACE_USER", user)];
    let out = crate::preview::run_written(pkg, dir, NAME, &test(&pkg.replace('-', "_"), root, &modules), &env)
        .map_err(|e| format!("the headless surface didn't render:\n{e}"))?;
    crate::preview::between(&out, BEGIN, END).ok_or_else(|| format!("the headless surface didn't render:\n{out}"))
}

/// The surface `tests::the_written_surface_shows_the_live_cells` makes, as
/// a test of ours: it compiles, and it renders on a served cell.
#[cfg(test)]
#[path = "headless_fixture.rs"]
#[rustfmt::skip]
mod written;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use aio::ui::prelude::*;

    #[aio::cell(shared, id = "shelf")]
    pub struct Shelf {
        pub books: Vec<String>,
    }

    #[aio::methods]
    impl Shelf {
        pub fn add(&mut self, title: String) {
            self.books.push(title);
        }
    }

    aio::client!(Shelf, shelf);

    #[allow(non_snake_case)]
    pub fn Books() -> View {
        use_live();
        let s = shelf();
        rsx! {
            h1 { "{s.books.len()} books" }
            ul {
                for b in s.books.iter() {
                    li { button { "{b}" } }
                }
            }
        }
    }

    #[test]
    fn every_module_with_a_client_is_found() {
        let f = |p: &str, t: &str| (PathBuf::from(p), t.to_string());
        let files = [
            f("lib.rs", "aio::client!(Notes, notes);\nmod ui;"),
            f("tags/mod.rs", "pub mod deep { client!(Tag, tag); }"),
            f("ui.rs", "pub fn App() {}"),
            f("bad.rs", "fn ("),
        ];
        assert_eq!(client_modules(&files), ["", "tags::deep"]);
        let t = test("notes", "App", &client_modules(&files));
        assert!(t.contains("let c0 = live(&notes::HANDLE") && t.contains("*notes::tags::deep::HANDLE.write() = c1;"));
        assert!(t.contains(".app(notes::App)"), "{t}");
    }

    #[test]
    fn the_written_surface_shows_the_live_cells() {
        assert_eq!(
            test("crate", "headless::tests::Books", &["headless::tests".into()]),
            include_str!("headless_fixture.rs")
        );
        // The running app: its cell, served at its door.
        let d = tempfile::tempdir().unwrap();
        let id = format!("headless-test-{}", std::process::id());
        let h = aio::Handle::<Shelf>::memory();
        h.add("Dune".into()).unwrap();
        h.add("Emma".into()).unwrap();
        let door = aio::sync::door(&id, d.path(), "shelf").unwrap();
        let l = {
            let _rt = aio::rt().enter();
            aio::ipc::Listener::bind(&door).unwrap()
        };
        aio::rt().spawn(aio::sync::serve_door(h.clone(), l, Default::default()));
        let shown = written::surface(&id, &d.path().to_string_lossy(), "ann");
        assert!(shown.contains("\"2 books\"") && shown.contains("\"Dune\"") && shown.contains("\"Emma\""), "{shown}");
        _ = std::fs::remove_dir_all(aio::paths::sockets(&id, d.path()).unwrap());
    }

    #[test]
    fn a_surface_needs_the_app_running_and_a_root_that_is_a_path() {
        let d = tempfile::tempdir().unwrap();
        assert!(run("x", d.path(), "App; drop", "x", d.path(), "u").unwrap_err().contains("a component's path"));
        assert!(run("x", d.path(), "App", "x", d.path(), "u").unwrap_err().contains("isn't running"));
    }

    #[test]
    fn its_output_is_told_from_the_tests() {
        let out = format!("running 1 test\n\n{BEGIN}\n  heading \"1 books\"\n{END}\ntest aio_surface ... ok\n");
        assert_eq!(crate::preview::between(&out, BEGIN, END).unwrap(), "  heading \"1 books\"");
    }
}

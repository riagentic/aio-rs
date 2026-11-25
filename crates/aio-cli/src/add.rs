//! `ax add <app> cell|server|page <name>` (aio's `am add`): a new cell, or a
//! module of server functions, written into an app and wired in: its
//! `mod` line in `src/lib.rs`, its source opened (or its functions served)
//! by each launcher in `app/src`, so the app builds and runs it at once.
//! A page is a crate of its own (`pages/<name>`), a dependency of the app's
//! library: `ax dev` recompiles that crate alone for an edit in it.
use std::path::{Path, PathBuf};

mod split;

const CELL: &str = include_str!("create/add_cell.rs");
const SERVER: &str = include_str!("create/add_server.rs");
const PAGE: &str = include_str!("create/add_page.rs");

/// What `ax add` adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Cell,
    Server,
}

impl Kind {
    pub fn of(word: &str) -> Result<Kind, String> {
        match word {
            "cell" => Ok(Kind::Cell),
            "server" => Ok(Kind::Server),
            _ => Err(format!("ax add <app> cell|server|page <name>: `{word}` is none of them")),
        }
    }
}

/// Rust's keywords: none can name a module.
const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn",
    "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self",
    "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "abstract",
    "become", "box", "do", "final", "macro", "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
];

/// `name`'s module and type names: `due-date` → `due_date`, `DueDate`
/// (pure). A name is a letter, then letters, digits, `-` or `_` (aio's
/// rule: it becomes a path and an identifier), and not a keyword.
pub fn names(name: &str) -> Result<(String, String), String> {
    let ok = name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        return Err(format!(
            "invalid name `{name}`: a letter, then letters, digits, - or _ (it becomes a file and an identifier)"
        ));
    }
    let snake = name.to_ascii_lowercase().replace('-', "_");
    if KEYWORDS.contains(&snake.as_str()) {
        return Err(format!("`{name}` is a Rust keyword: name it otherwise"));
    }
    let pascal: String =
        snake.split('_').filter(|w| !w.is_empty()).map(|w| w[..1].to_ascii_uppercase() + &w[1..]).collect();
    Ok((snake, pascal))
}

/// The new cell's file (pure): the template's names replaced; `ui`: with
/// its views' `client!` (an app with a window).
pub fn cell_file(snake: &str, pascal: &str, ui: bool) -> String {
    let text = CELL.replace("Thing", "\u{0}").replace("thing", snake).replace('\u{0}', pascal);
    match ui {
        true => text,
        false => text
            .lines()
            .filter(|l| !["client!", "use_live", "act!("].iter().any(|w| l.contains(w)))
            .map(|l| format!("{l}\n"))
            .collect::<String>()
            .replace("}\n\n\n", "}\n\n"),
    }
}

/// The new server functions' file (pure).
pub fn server_file(snake: &str) -> String {
    SERVER.replace("thing", snake)
}

/// The one-line `[workspace] members` and `[dependencies]` aio of `lines` (a manifest).
fn spots(lines: &[String]) -> Result<(usize, usize), String> {
    let (mut section, mut members, mut aio) = (String::new(), None, None);
    for (i, line) in lines.iter().enumerate() {
        let l = line.trim();
        if l.starts_with('[') {
            section = l.to_string();
        } else if section == "[workspace]" && l.starts_with("members = [") && l.ends_with(']') {
            members = Some(i);
        } else if section == "[dependencies]" && (l.starts_with("aio ") || l.starts_with("aio=")) {
            aio = Some(i);
        }
    }
    let members =
        members.ok_or("Cargo.toml has no one-line [workspace] members = [..]: add the page's crate yourself")?;
    Ok((members, aio.ok_or("no one-line aio dependency under [dependencies] in Cargo.toml")?))
}

/// The package `manifest` names (pure).
fn package_of(manifest: &str) -> Result<String, String> {
    let t: toml::Table = manifest.parse().map_err(|e| format!("Cargo.toml: {e}"))?;
    let name = t.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str());
    name.map(String::from).ok_or_else(|| "Cargo.toml names no package".into())
}

/// `manifest` with the crate in `dir` a workspace member and `dep` (its
/// line) a dependency of the library, beside its aio (pure).
fn with_member(manifest: &str, dir: &str, dep: &str) -> Result<String, String> {
    let mut lines: Vec<String> = manifest.lines().map(String::from).collect();
    let (members, aio) = spots(&lines)?;
    lines[aio].push_str(&format!("\n{dep}"));
    let m = &mut lines[members];
    m.insert_str(m.len() - 1, &format!(", \"{dir}\""));
    Ok(lines.join("\n") + "\n")
}

/// A new page's crate (pure): its manifest and source, and the app's
/// `manifest` with it a workspace member and a dependency of the library,
/// on the app's own aio-rs (its line, a path two folders deeper) and its
/// core crate (`core`: its package, and what the page shows of a cell).
pub fn page_crate(
    manifest: &str,
    snake: &str,
    pascal: &str,
    core: Option<(&str, Option<&split::Reading>)>,
) -> Result<[String; 3], String> {
    let t: toml::Table = manifest.parse().map_err(|e| format!("Cargo.toml: {e}"))?;
    let package = package_of(manifest)?;
    if t.get("dependencies").and_then(|d| d.get(snake)).is_some() {
        return Err(format!("the app depends on a `{snake}` already: name the page otherwise"));
    }
    let lines: Vec<String> = manifest.lines().map(String::from).collect();
    let (_, aio) = spots(&lines)?;
    let name = format!("{package}-{}", snake.replace('_', "-"));
    let dep = format!("{snake} = {{ package = \"{name}\", path = \"pages/{snake}\" }}");
    let app = with_member(manifest, &format!("pages/{snake}"), &dep)?;
    let shared = match t.get("workspace").and_then(|w| w.get("package")).is_some() {
        true => "edition.workspace = true\npublish.workspace = true",
        false => "edition = \"2024\"\npublish = false",
    };
    let core_dep = core.map_or(String::new(), |(p, _)| format!("{p} = {{ path = \"../../{}\" }}\n", split::DIR));
    let own = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n{shared}\n\n[dependencies]\n{}\n{core_dep}",
        deeper(&lines[aio], 2)
    );
    Ok([app, own, page_source(snake, pascal, core.and_then(|(_, r)| r))])
}

/// A new page's source (pure); `reading`: it reads a cell of the core crate.
pub fn page_source(snake: &str, pascal: &str, reading: Option<&split::Reading>) -> String {
    let source = PAGE.replace("Thing", "\u{0}").replace("thing", snake).replace('\u{0}', pascal);
    let Some(r) = reading else { return source };
    let prelude = format!(
        "//! The app's cells are in its core crate ({}/): `{}` brings a cell's\n\
         //! `part`, `act!` and the view API, as `crate::prelude` does in the library.\nuse {}::*;\n",
        split::DIR,
        r.prelude,
        r.prelude
    );
    let source = source.replace("use aio::ui::prelude::*;\n", &prelude);
    let Some((f, list)) = &r.field else { return source };
    let read = format!(
        "    // A field of the cell: what reads it runs again only when it changes.\n    let {f} = part(|c| &c.{f});\n"
    );
    let shown = match list {
        true => format!("            p {{ {{format!(\"{f}: {{}}\", {f}.with(Vec::len))}} }}\n"),
        false => format!("            p {{ \"{f}: {{{f}}}\" }}\n"),
    };
    let h2 = format!("            h2 {{ \"{pascal}\" }}\n");
    source.replacen("    rsx! {\n", &format!("{read}    rsx! {{\n"), 1).replacen(&h2, &format!("{h2}{shown}"), 1)
}

/// A dependency's line `up` folders deeper (pure): its relative path too.
pub(crate) fn deeper(line: &str, up: usize) -> String {
    match line.find("path = \"") {
        Some(i) if !line[i + 8..].starts_with('/') && !line[i + 8..].get(1..).is_some_and(|r| r.starts_with(':')) => {
            format!("{}{}{}", &line[..i + 8], "../".repeat(up), &line[i + 8..])
        }
        _ => line.to_string(),
    }
}

/// `lib` with `pub mod <snake>;` among its top-level `mod x;` lines in
/// rustfmt's order, else after its crate docs (pure); `native`: on native builds only (a browser build
/// serves nothing). None: it has the module already.
pub fn with_mod(lib: &str, snake: &str, native: bool) -> Option<String> {
    let has = |l: &str| {
        let l = l.trim_start_matches("pub ").trim_start_matches("pub(crate) ");
        l == format!("mod {snake};") || l.starts_with(&format!("mod {snake} "))
    };
    if lib.lines().any(has) {
        return None;
    }
    let lines: Vec<&str> = lib.lines().collect();
    fn name(l: &str) -> Option<&str> {
        l.strip_prefix("pub ").unwrap_or(l).strip_prefix("mod ")?.strip_suffix(';')
    }
    // rustfmt's order: before the first one named after it (and its attributes), else after the last.
    let after = lines
        .iter()
        .position(|l| name(l).is_some_and(|n| n > snake))
        .map(|i| i - lines[..i].iter().rev().take_while(|l| l.starts_with("#[")).count());
    let at = match (after, lines.iter().rposition(|l| name(l).is_some())) {
        (Some(i), _) => i,
        (None, Some(i)) => i + 1,
        (None, None) => lines.iter().take_while(|l| l.starts_with("//!")).count(),
    };
    let cfg = if native { "#[cfg(not(target_arch = \"wasm32\"))]\n" } else { "" };
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    out.insert(at, format!("{cfg}pub mod {snake};"));
    Some(out.join("\n") + "\n")
}

/// The text inside the parentheses that open at `at` (`at` is the `(`).
fn inside(line: &str, at: usize) -> Option<&str> {
    let mut depth = 0;
    for (i, c) in line[at..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 1 => return Some(&line[at + 1..at + i]),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The path of the app's library in a launcher's line (`app::` in
/// `app::SOURCE.set(` or `aio::Source::<app::Notes>::`; `""` inside the
/// library itself).
fn lib_path(line: &str) -> String {
    if let Some(i) = line.find("SOURCE.set(") {
        let head = &line[..i];
        let start = head.rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')).map_or(0, |s| s + 1);
        return head[start..].to_string();
    }
    let ty = line.split("aio::Source::<").nth(1).and_then(|r| r.split(">::").next()).unwrap_or_default();
    ty.rsplit_once("::").map_or(String::new(), |(p, _)| format!("{p}::"))
}

/// `launcher` with the new cell's source opened beside the first one it
/// opens (pure): `ui`, set into its `client!` source; else held for the
/// run. None: it opens none, or opens this one already.
pub fn wire_cell(launcher: &str, snake: &str, pascal: &str, ui: bool) -> Option<String> {
    let lines: Vec<&str> = launcher.lines().collect();
    let i = lines.iter().position(|l| {
        l.contains("aio::Source::")
            && (l.contains("for_app") || l.contains("for_server") || l.contains("Handle::saved("))
    })?;
    let line = lines[i];
    let lib = lib_path(line);
    if launcher.contains(&format!("{lib}{snake}::")) {
        return None;
    }
    let indent = &line[..line.len() - line.trim_start().len()];
    let new = if line.contains("Handle::saved(") {
        let args = inside(line, line.find("Handle::saved(")? + "Handle::saved".len())?;
        format!("{indent}_ = {lib}{snake}::SOURCE.set(aio::Source::Local(aio::Handle::saved({args})));")
    } else {
        let ctor = ["for_app_with", "for_app", "for_server"].into_iter().find(|c| line.contains(&format!("{c}(")))?;
        let args = inside(line, line.find(&format!("{ctor}("))? + ctor.len())?;
        // A cell's own source takes no app routes: those stay on the first one.
        let (ctor, args) = match ctor {
            "for_app_with" => ("for_app", args.rsplit_once(',')?.0.trim()),
            c => (c, args),
        };
        match ui {
            true => format!("{indent}_ = {lib}{snake}::SOURCE.set(aio::Source::{ctor}({args})?);"),
            false => format!("{indent}let _{snake} = aio::Source::<{lib}{snake}::{pascal}>::{ctor}({args})?;"),
        }
    };
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    out.insert(i + 1, new);
    Some(out.join("\n") + "\n")
}

/// `launcher` with the new module's server functions served by the first
/// source it opens (pure): `for_app(a, b)` becomes `for_app_with(a, b,
/// <lib>::<snake>::http(..))`, an `Http` already there wrapped. Err: it
/// opens none that can serve them (a `for_server` source takes none).
pub fn wire_server(launcher: &str, snake: &str) -> Result<Option<String>, String> {
    let lines: Vec<&str> = launcher.lines().collect();
    let Some(i) = lines.iter().position(|l| l.contains("aio::Source::") && l.contains("for_app")) else {
        return Err("it opens no source with aio::Source::for_app".into());
    };
    let line = lines[i];
    let lib = lib_path(line);
    let http = format!("{lib}{snake}::http(");
    if launcher.contains(&http) {
        return Ok(None);
    }
    let new = match line.find("for_app_with(") {
        Some(at) => {
            let args = inside(line, at + "for_app_with".len()).ok_or("an unclosed for_app_with(")?;
            let (head, last) = args.rsplit_once(',').ok_or("for_app_with with no Http")?;
            let wrapped = format!("{head}, {http}{})", last.trim());
            line.replacen(args, &wrapped, 1)
        }
        None => {
            let at = line.find("for_app(").ok_or("no for_app(")?;
            let args = inside(line, at + "for_app".len()).ok_or("an unclosed for_app(")?;
            line.replacen(
                &format!("for_app({args})"),
                &format!("for_app_with({args}, {http}aio::http::Http::default()))"),
                1,
            )
        }
    };
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    out[i] = new;
    Ok(Some(out.join("\n") + "\n"))
}

/// `manifest` (the library's Cargo.toml) with aio's `server` feature on its
/// `aio` dependency (pure); None: a feature that brings it is on already.
/// An app with a browser build gets it on its native builds alone, in a
/// table of their own: the feature doesn't build for wasm32.
pub fn with_server_feature(manifest: &str) -> Result<Option<String>, String> {
    const NATIVE: &str = "[target.'cfg(not(target_arch = \"wasm32\"))'.dependencies]";
    let web = manifest.contains("[target.'cfg(target_arch = \"wasm32\")'.dependencies]");
    let mut section = "";
    let mut aio = None;
    for (i, line) in manifest.lines().enumerate() {
        let t = line.trim();
        if t.starts_with('[') {
            section = t;
            continue;
        }
        if !(section == "[dependencies]" || section == NATIVE) || !(t.starts_with("aio ") || t.starts_with("aio=")) {
            continue;
        }
        if ["\"server\"", "\"tls\"", "\"net\"", "\"oidc\"", "\"br\""].iter().any(|f| t.contains(f)) {
            return Ok(None);
        }
        if section == "[dependencies]" {
            aio = Some((i, line));
        }
    }
    let (i, line) = aio.ok_or("no aio dependency under [dependencies] in its Cargo.toml")?;
    // Its source alone (a native table adds to it), or the line with the feature added.
    let line = match web {
        true => match (line.find(", features = ["), line.rfind('}')) {
            (Some(at), Some(end)) => format!("{}{}", &line[..at], &line[end..]),
            _ => line.to_string(),
        },
        false => line.to_string(),
    };
    let new = match (line.find("features = ["), line.rfind('}')) {
        (Some(at), _) => {
            let open = at + "features = [".len();
            let sep = if line[open..].starts_with(']') { "" } else { ", " };
            format!("{}\"server\"{sep}{}", &line[..open], &line[open..])
        }
        (None, Some(end)) => format!("{}, features = [\"server\"] {}", line[..end].trim_end(), &line[end..]),
        (None, None) => {
            return Err(format!(
                "its aio dependency isn't one line: add features = [\"server\"] to it: {}",
                line.trim()
            ));
        }
    };
    if web {
        return Ok(Some(format!(
            "{}\n\n# Server functions (ax add server): on the native builds; a page calls them.\n{NATIVE}\n{new}\n",
            manifest.trim_end()
        )));
    }
    let mut out: Vec<&str> = manifest.lines().collect();
    out[i] = &new;
    Ok(Some(out.join("\n") + "\n"))
}

/// The launchers under `root/app/src` (`main.rs`, Android's `lib.rs`).
fn launchers(root: &Path) -> Vec<PathBuf> {
    ["main.rs", "lib.rs"].iter().map(|f| root.join("app").join("src").join(f)).filter(|f| f.is_file()).collect()
}

/// `ax add [app] cell|server <name>`: written into the app at `root`,
/// wired in, said (`json`: one object).
pub fn run(root: &Path, kind: Kind, name: &str, json: bool) -> Result<String, String> {
    let (snake, pascal) = names(name)?;
    let read = |f: &Path| std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()));
    let lib_rs = root.join("src").join("lib.rs");
    let lib = read(&lib_rs).map_err(|e| format!("{e}: ax add writes into an app's library (src/lib.rs)"))?;
    let core = split::existing(root).map(|_| root.join(split::DIR).join("src"));
    for src in std::iter::once(root.join("src")).chain(core.clone()) {
        if src.join(format!("{snake}.rs")).exists() || src.join(&snake).join("mod.rs").exists() {
            return Err(format!("{}: the app has a module {snake} already", src.join(&snake).display()));
        }
    }
    let lib_new = with_mod(&lib, &snake, kind == Kind::Server)
        .ok_or_else(|| format!("src/lib.rs declares a module {snake} already"))?;
    // A cell of an app with a core crate goes there, where its pages read it.
    let core = core.filter(|_| kind == Kind::Cell);
    let file = core.as_ref().unwrap_or(&root.join("src")).join(format!("{snake}.rs"));
    let core_lib = core.as_ref().map(|c| c.join("lib.rs"));
    let core_text = core_lib.as_ref().map(|f| read(f)).transpose()?;
    let ui = lib.contains("aio::client!(") || core_text.as_ref().is_some_and(|t| t.contains("aio::client!("));
    // Every change worked out first: nothing is written unless all of it can be.
    let mut writes: Vec<(PathBuf, String)> = vec![];
    let mut notes: Vec<String> = vec![];
    let launchers = launchers(root);
    match kind {
        Kind::Cell => {
            writes.push((file.clone(), cell_file(&snake, &pascal, ui)));
            // The browser build's source, in the library itself.
            match (core_lib, core_text) {
                (Some(f), Some(t)) => {
                    let with = with_mod(&t, &snake, false)
                        .ok_or_else(|| format!("{} declares a module {snake} already", rel(root, &f)))?;
                    writes.push((f, with));
                    writes.extend(wire_cell(&lib, &snake, &pascal, ui).map(|t| (lib_rs, t)));
                }
                _ => writes.push((lib_rs, wire_cell(&lib_new, &snake, &pascal, ui).unwrap_or(lib_new))),
            }
            for l in &launchers {
                match wire_cell(&read(l)?, &snake, &pascal, ui) {
                    Some(t) => writes.push((l.clone(), t)),
                    None => {
                        notes.push(format!("{}: no aio::Source opened there: open {snake}'s yourself", rel(root, l)))
                    }
                }
            }
            if ui {
                notes.push(format!("its views update once the root view calls {snake}::use_live()"));
            }
        }
        Kind::Server => {
            writes.push((file.clone(), server_file(&snake)));
            writes.push((lib_rs, lib_new));
            let manifest = root.join("Cargo.toml");
            if let Some(t) = with_server_feature(&read(&manifest)?)? {
                writes.push((manifest, t));
                notes.push("aio's `server` feature turned on (Cargo.toml): server functions need it".into());
            }
            for l in &launchers {
                match wire_server(&read(l)?, &snake) {
                    Ok(Some(t)) => writes.push((l.clone(), t)),
                    Ok(None) => {}
                    Err(e) => notes.push(format!(
                        "{}: {e}: serve them yourself (Source::for_app_with(.., {snake}::http(Http::default())))",
                        rel(root, l)
                    )),
                }
            }
        }
    }
    if launchers.is_empty() {
        notes.push(format!("no launcher in app/src: open {snake} where the app starts"));
    }
    for (f, text) in &writes {
        std::fs::write(f, text).map_err(|e| format!("{}: {e}", f.display()))?;
    }
    let created = rel(root, &file);
    let changed: Vec<String> = writes.iter().skip(1).map(|(f, _)| rel(root, f)).collect();
    if json {
        return Ok(serde_json::json!({ "created": created, "changed": changed, "notes": notes }).to_string());
    }
    let mut out = vec![format!("created {created}")];
    out.extend(changed.iter().map(|c| format!("changed {c}")));
    out.extend(notes);
    let call = match kind {
        Kind::Cell => format!("ax call <app> {snake} add 1"),
        Kind::Server => format!("ax call <app> --fn {snake}.list"),
    };
    out.push(format!("next: ax test, then {call}"));
    Ok(out.join("\n"))
}

/// Rust's own crates: a page named so would hide one from the app.
const RUSTS: [&str; 5] = ["core", "std", "alloc", "proc_macro", "test"];

/// `ax add [app] page <name>`: its crate under `pages/`, on the app's core
/// crate, said. The first page of an app whose library holds its cells
/// moves them there first ([`split`]): asked (`yes`: not; `json`: the plan
/// said, nothing done), tried on a copy, then written; else nothing changes.
pub fn page(root: &Path, name: &str, yes: bool, json: bool) -> Result<String, String> {
    let (snake, pascal) = names(name)?;
    let (snake, pascal) = (snake.as_str(), pascal.as_str());
    if RUSTS.contains(&snake) {
        return Err(format!("`{name}` names one of Rust's own crates, which it would hide: name the page otherwise"));
    }
    let dir = format!("pages/{snake}");
    if root.join(&dir).exists() {
        return Err(format!("{dir}: there already"));
    }
    let read = |f: &str| std::fs::read_to_string(root.join(f)).map_err(|e| format!("{f}: {e}"));
    let manifest = read("Cargo.toml")?;
    let existing = split::existing(root);
    let plan = match existing {
        Some(_) => None,
        None => split::plan(root, &package_of(&manifest)?, &read("src/lib.rs")?, &manifest)?,
    };
    let core = existing.or_else(|| plan.as_ref().map(|p| p.package.clone()));
    let core_lib = format!("{}/src/lib.rs", split::DIR);
    // The core crate's source, as it is or is to be.
    let core_file = |f: &str| match &plan {
        Some(p) => p.writes.iter().find(|(w, _)| *w == format!("{}/src/{f}", split::DIR)).map(|(_, b)| b.clone()),
        None => std::fs::read(root.join(split::DIR).join("src").join(f)).ok(),
    };
    let text = |f: &str| core_file(f).and_then(|b| String::from_utf8(b).ok());
    let reading = core.as_ref().and_then(|c| split::reading(&c.replace('-', "_"), &text("lib.rs")?, &text));
    let base = match &plan {
        Some(p) => with_member(&manifest, split::DIR, &format!("{} = {{ path = \"{}\" }}", p.package, split::DIR))?,
        None => manifest.clone(),
    };
    let [app, own, source] = page_crate(&base, snake, pascal, core.as_deref().map(|c| (c, reading.as_ref())))?;
    let created = format!("{dir}/src/lib.rs");
    let mut writes = vec![
        (format!("{dir}/Cargo.toml"), own.into_bytes()),
        (created.clone(), source.into_bytes()),
        ("Cargo.toml".to_string(), app.into_bytes()),
    ];
    let show = format!("show it from the app's view: {snake}::{pascal} {{}}");
    let Some(plan) = plan else {
        write(root, &writes, &[])?;
        if json {
            return Ok(
                serde_json::json!({ "created": created, "changed": ["Cargo.toml"], "notes": [show] }).to_string()
            );
        }
        return Ok(format!("created {created}\nchanged Cargo.toml\n{show}\nnext: ax dev <app>"));
    };
    // The pages made before depend on it too.
    for (_, path) in split::pages(&manifest) {
        let f = format!("{path}/Cargo.toml");
        writes.push((f.clone(), split::with_core_dep(&read(&f)?, &plan.package, 2).into_bytes()));
    }
    writes.extend(plan.writes.iter().cloned());
    if !yes {
        let said = plan.said(json);
        if json {
            let next = format!("ax add <app> page {name} --yes");
            return Ok(serde_json::json!({ "done": false, "core": said, "next": next }).to_string());
        }
        eprintln!("{}", said.as_str().unwrap_or_default());
        match crate::fetched::ask_terminal("Make it?") {
            None => return Err("nothing changed: nobody here to ask; read the above, then again with --yes".into()),
            Some(false) => return Err("nothing changed".into()),
            Some(true) => {}
        }
    }
    let trial = split::Trial::of(root)?;
    write(&trial.0, &writes, &plan.removes)?;
    eprintln!("ax add: the app with its cells in {}/, checked on a copy (cargo check)", split::DIR);
    let unused = split::check(&trial.0, &core_lib).map_err(|e| {
        format!(
            "nothing changed: the app with its cells in {}/ doesn't build:\n\n{e}\n\nmake what its views use of them \
             pub, then ax add again; or move them yourself (docs/clients/ax.md#ax-add)",
            split::DIR
        )
    })?;
    // Imports of the library the cells didn't need, dropped from the copy's.
    if let Some((_, b)) = writes.iter_mut().find(|(f, _)| *f == core_lib) {
        *b = split::edited(&String::from_utf8_lossy(b), &unused).into_bytes();
    }
    writes.extend(std::fs::read(trial.0.join("Cargo.lock")).ok().map(|l| ("Cargo.lock".to_string(), l)));
    drop(trial);
    write(root, &writes, &plan.removes)?;
    let krate = plan.package.replace('-', "_");
    let mut out = vec![
        format!("created {created}"),
        "changed Cargo.toml".into(),
        format!("made {}/ ({}): {}", split::DIR, plan.package, plan.moved.join(", ")),
        format!("changed src/lib.rs: pub use {krate}::*; (each path to them as it was)"),
    ];
    out.extend(plan.removes.iter().map(|r| format!("moved {r} to {}/{r}", split::DIR)));
    out.extend(split::pages(&manifest).iter().map(|(_, p)| format!("changed {p}/Cargo.toml")));
    if json {
        return Ok(serde_json::json!({ "created": created, "changed": ["Cargo.toml", "src/lib.rs"],
            "core": plan.said(true), "notes": [show] })
        .to_string());
    }
    out.extend([show, "next: ax dev <app>".into()]);
    Ok(out.join("\n"))
}

/// `writes` into the app at `root` (each file replaced whole, its folder
/// made), then `removes` removed, with the folders that leaves empty.
fn write(root: &Path, writes: &[(String, Vec<u8>)], removes: &[String]) -> Result<(), String> {
    for (f, bytes) in writes {
        let p = root.join(f);
        std::fs::create_dir_all(p.parent().unwrap_or(root)).map_err(|e| format!("{f}: {e}"))?;
        aio::paths::write_atomic(&p, bytes, false).map_err(|e| format!("{f}: {e}"))?;
    }
    for f in removes {
        std::fs::remove_file(root.join(f)).map_err(|e| format!("{f}: {e}"))?;
        let src = root.join("src");
        let mut d = root.join(f).parent().map(Path::to_path_buf);
        while let Some(p) = d.filter(|p| p.starts_with(&src) && *p != src && std::fs::remove_dir(p).is_ok()) {
            d = p.parent().map(Path::to_path_buf);
        }
    }
    Ok(())
}

/// `f` relative to `root`, with `/`.
fn rel(root: &Path, f: &Path) -> String {
    f.strip_prefix(root).unwrap_or(f).to_string_lossy().replace('\\', "/")
}

// The files `ax add` writes, compiled and tested as the app's own.
#[cfg(test)]
#[path = "create/add_cell.rs"]
mod add_cell;
#[cfg(test)]
#[path = "create/add_page.rs"]
mod add_page;
#[cfg(test)]
#[path = "create/add_server.rs"]
mod add_server;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_a_module_and_a_type() {
        assert_eq!(names("due-date"), Ok(("due_date".into(), "DueDate".into())));
        assert_eq!(names("Inbox"), Ok(("inbox".into(), "Inbox".into())));
        assert_eq!(names("a__b"), Ok(("a__b".into(), "AB".into())));
        for bad in ["../etc/x", "1st", "a b", "x}", "", "-x"] {
            assert!(names(bad).unwrap_err().contains("invalid name"), "{bad}");
        }
        assert!(names("type").unwrap_err().contains("keyword"));
        assert!(names("Self").is_err() && names("self").is_err(), "lowercased first");
    }

    #[test]
    fn the_cell_file_takes_the_names_and_loses_its_views_without_a_window() {
        let ui = cell_file("something", "Something", true);
        assert!(ui.contains("pub struct Something {") && ui.contains("id = \"something\""), "{ui}");
        assert!(ui.contains("aio::client!(Something, something);"));
        assert!(!ui.contains("Thing") && !ui.contains("\"thing\""), "{ui}");
        let plain = cell_file("tally", "Tally", false);
        assert!(!plain.contains("client!") && !plain.contains("use_live"), "{plain}");
        assert!(plain.contains("impl Tally {") && plain.contains("Handle::<Tally>::memory()"));
        assert!(!plain.contains("\n\n\n"), "{plain}");
        let s = server_file("tools");
        assert!(s.contains("ServerFn::new(\"tools.list\")") && s.contains("--fn tools.list") && !s.contains("thing"));
    }

    #[test]
    fn the_mod_line_goes_after_the_last_mod_or_the_crate_docs() {
        let lib = "//! The app.\n//! More.\nuse aio::ui::prelude::*;\n\nfn x() {}\n";
        assert_eq!(
            with_mod(lib, "tally", false).unwrap(),
            "//! The app.\n//! More.\npub mod tally;\nuse aio::ui::prelude::*;\n\nfn x() {}\n"
        );
        let lib = "//! A.\npub mod a;\nmod b;\n\nfn x() {}\n#[cfg(test)]\nmod tests {\n}\n";
        assert_eq!(
            with_mod(lib, "c", true).unwrap(),
            "//! A.\npub mod a;\nmod b;\n#[cfg(not(target_arch = \"wasm32\"))]\npub mod c;\n\nfn x() {}\n#[cfg(test)]\nmod tests {\n}\n"
        );
        assert_eq!(with_mod(lib, "a", false), None);
        assert_eq!(with_mod("pub(crate) mod a {\n}\n", "a", false), None);
        // In rustfmt's order (cargo fmt --check, ax check, passes), its attributes going with it.
        let lib = "//! A.\npub mod tasks;\n";
        let cfg = "#[cfg(not(target_arch = \"wasm32\"))]\n";
        assert_eq!(with_mod(lib, "api", true).unwrap(), format!("//! A.\n{cfg}pub mod api;\npub mod tasks;\n"));
        let lib = format!("//! A.\npub mod a;\n{cfg}pub mod c;\n");
        assert_eq!(with_mod(&lib, "b", false).unwrap(), format!("//! A.\npub mod a;\npub mod b;\n{cfg}pub mod c;\n"));
    }

    #[test]
    fn each_launcher_opens_the_new_cell_beside_its_first() {
        let main = crate::create::APP_MAIN;
        let w = wire_cell(main, "tally", "Tally", true).unwrap();
        assert!(w.contains("    _ = app::SOURCE.set(aio::Source::for_app(&cfg, &dirs)?);\n    _ = app::tally::SOURCE.set(aio::Source::for_app(&cfg, &dirs)?);\n"), "{w}");
        assert_eq!(wire_cell(&w, "tally", "Tally", true), None, "once");
        let android = wire_cell(crate::create::APP_ANDROID, "tally", "Tally", true).unwrap();
        assert!(android.contains("_ = app::tally::SOURCE.set(aio::Source::for_app(cfg, dirs)?);"), "{android}");
        let cli = wire_cell(crate::create::CLI_MAIN, "tally", "Tally", false).unwrap();
        assert!(cli.contains("let _tally = aio::Source::<app::tally::Tally>::for_app(&cfg, &dirs)?;"), "{cli}");
        let service = wire_cell(crate::create::SERVICE_MAIN, "tally", "Tally", false).unwrap();
        assert!(
            service.contains("let _tally = aio::Source::<app::tally::Tally>::for_server(&cfg, &dirs)?;"),
            "{service}"
        );
        let web = "    _ = SOURCE.set(aio::Source::Local(aio::Handle::saved(id)));\n";
        assert_eq!(
            wire_cell(web, "tally", "Tally", true).unwrap(),
            format!("{web}    _ = tally::SOURCE.set(aio::Source::Local(aio::Handle::saved(id)));\n")
        );
        let with = "_ = app::SOURCE.set(aio::Source::for_app_with(&cfg, &dirs, app::http())?);";
        assert!(
            wire_cell(with, "t", "T", true)
                .unwrap()
                .contains("_ = app::t::SOURCE.set(aio::Source::for_app(&cfg, &dirs)?);")
        );
        assert_eq!(wire_cell("fn main() {}\n", "t", "T", true), None);
    }

    #[test]
    fn the_launcher_serves_the_new_functions() {
        let w = wire_server(crate::create::APP_MAIN, "tools").unwrap().unwrap();
        assert!(
            w.contains("aio::Source::for_app_with(&cfg, &dirs, app::tools::http(aio::http::Http::default()))?"),
            "{w}"
        );
        let twice = wire_server(&w, "more").unwrap().unwrap();
        assert!(
            twice.contains("for_app_with(&cfg, &dirs, app::more::http(app::tools::http(aio::http::Http::default())))?"),
            "{twice}"
        );
        assert_eq!(wire_server(&w, "tools"), Ok(None));
        assert!(wire_server(crate::create::SERVICE_MAIN, "tools").unwrap_err().contains("for_app"));
    }

    /// An app with a browser build (the window templates) gets `server` on
    /// its native builds alone: the feature doesn't build for wasm32 (`ax
    /// check`'s web step, `ax dev --web`, `ax build --web` failed after it).
    #[test]
    fn an_app_with_a_page_gets_the_server_feature_natively() {
        let all = crate::create::files(&crate::create::New::of("x")).unwrap();
        let m = &all.iter().find(|(f, _)| f == "Cargo.toml").unwrap().1;
        let on = with_server_feature(m).unwrap().unwrap();
        let native = "[target.'cfg(not(target_arch = \"wasm32\"))'.dependencies]\n\
                      aio = { package = \"aio-rs\", git = \"https://github.com/riagentic/aio-rs\", tag = \"v";
        assert!(on.contains(native) && on.contains("\", features = [\"server\"] }\n"), "{on}");
        let dependencies = on.lines().skip_while(|l| *l != "[dependencies]").nth(1).unwrap();
        assert!(dependencies.ends_with("features = [\"kit\"] }"), "every target's as it was: {dependencies}");
        assert_eq!(with_server_feature(&on), Ok(None), "once");
    }

    #[test]
    fn the_server_feature_is_turned_on_once() {
        let m = "[package]\nname = \"x\"\n\n[dependencies]\naio = { package = \"aio-rs\", git = \"g\", tag = \"v1\", features = [\"kit\"] }\n\n[dev-dependencies]\naio = { package = \"aio-rs\", features = [\"testing\"] }\n";
        let on = with_server_feature(m).unwrap().unwrap();
        assert!(on.contains("features = [\"server\", \"kit\"] }\n\n[dev"), "{on}");
        assert!(on.contains("features = [\"testing\"]"), "dev-dependencies untouched");
        assert_eq!(with_server_feature(&on), Ok(None));
        let bare = "[dependencies]\naio = { package = \"aio-rs\", git = \"g\" }\n";
        assert_eq!(
            with_server_feature(bare).unwrap().unwrap(),
            "[dependencies]\naio = { package = \"aio-rs\", git = \"g\", features = [\"server\"] }\n"
        );
        assert_eq!(with_server_feature("[dependencies]\naio = { features = [\"net\"] }\n"), Ok(None));
        assert!(with_server_feature("[dependencies]\nx = 1\n").is_err());
    }

    #[test]
    fn a_page_is_a_crate_of_its_own_on_the_apps_aio_rs() {
        let all = crate::create::files(&crate::create::New::of("notes")).unwrap();
        let manifest = &all.iter().find(|(f, _)| f == "Cargo.toml").unwrap().1;
        let released =
            format!("git = \"https://github.com/riagentic/aio-rs\", tag = \"v{}\"", env!("CARGO_PKG_VERSION"));
        let manifest = manifest.replace(&released, "path = \"../a/aio\"");
        let [app, own, source] = page_crate(&manifest, "due_date", "DueDate", None).unwrap();
        assert!(app.contains("members = [\".\", \"app\", \"pages/due_date\"]"), "{app}");
        assert!(app.contains("\ndue_date = { package = \"notes-due-date\", path = \"pages/due_date\" }\n"), "{app}");
        assert_eq!(
            app.replace("\ndue_date = { package = \"notes-due-date\", path = \"pages/due_date\" }", "")
                .replace(", \"pages/due_date\"", ""),
            manifest
        );
        assert!(
            own.starts_with("[package]\nname = \"notes-due-date\"\nversion = \"0.1.0\"\nedition.workspace = true\n"),
            "{own}"
        );
        assert!(
            own.contains("aio = { package = \"aio-rs\", path = \"../../../a/aio\", features = [\"kit\"] }"),
            "{own}"
        );
        assert!(
            source.contains("pub fn DueDate() -> View")
                && source.contains("`due_date::DueDate {}`")
                && !source.contains("thing")
        );
        assert!(page_crate(&app, "due_date", "DueDate", None).unwrap_err().contains("already"));
        assert!(page_crate("[package]\nname = \"x\"\n", "p", "P", None).unwrap_err().contains("members"));
        let ui = aio::testing::TestUi::new(add_page::Thing);
        assert!(ui.html().contains("<h2>Thing</h2>"), "{}", ui.html());
        // On the app's core crate: its prelude, a field of its cell shown.
        let r = split::Reading { prelude: "notes_core::prelude".into(), field: Some(("count".into(), false)) };
        let [_, own, source] = page_crate(&manifest, "due_date", "DueDate", Some(("notes-core", Some(&r)))).unwrap();
        assert!(own.ends_with("features = [\"kit\"] }\nnotes-core = { path = \"../../core\" }\n"), "{own}");
        assert!(!source.contains("use aio::ui::prelude::*;") && source.contains("\nuse notes_core::prelude::*;\n"));
        assert!(source.contains("    let count = part(|c| &c.count);\n    rsx! {\n"), "{source}");
        assert!(source.contains("            h2 { \"DueDate\" }\n            p { \"count: {count}\" }\n"), "{source}");
        let r = split::Reading { prelude: "notes_core::todo::prelude".into(), field: Some(("items".into(), true)) };
        let list = page_source("x", "X", Some(&r));
        assert!(list.contains("p { {format!(\"items: {}\", items.with(Vec::len))} }"), "{list}");
        let r = split::Reading { prelude: "notes_core::prelude".into(), field: None };
        assert!(!page_source("x", "X", Some(&r)).contains("part("));
    }

    /// A page named `core` (or `std`) is refused: its crate would hide Rust's
    /// own from the app, and `core` is the cells' crate's folder.
    #[test]
    fn a_page_may_not_take_a_name_of_rusts_own_crates() {
        let d = tempfile::tempdir().unwrap();
        for n in ["core", "std", "alloc"] {
            assert!(page(d.path(), n, true, false).unwrap_err().contains("Rust's own crates"), "{n}");
        }
    }

    #[test]
    fn an_app_gets_its_cell_and_functions_written_and_wired_or_nothing() {
        let d = tempfile::tempdir().unwrap();
        let all = crate::create::files(&crate::create::New::of("notes")).unwrap();
        for (f, text) in &all {
            let p = d.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let said = run(d.path(), Kind::Cell, "tally", false).unwrap();
        assert!(said.starts_with("created src/tally.rs\nchanged src/lib.rs\nchanged app/src/main.rs"), "{said}");
        let get = |f: &str| std::fs::read_to_string(d.path().join(f)).unwrap();
        assert!(get("src/lib.rs").contains("pub mod tally;") && get("src/lib.rs").contains("_ = tally::SOURCE.set("));
        assert!(
            get("app/src/main.rs").contains("app::tally::SOURCE")
                && get("app/src/lib.rs").contains("app::tally::SOURCE")
        );
        let before = get("src/lib.rs");
        assert!(run(d.path(), Kind::Cell, "tally", false).unwrap_err().contains("already"));
        assert_eq!(get("src/lib.rs"), before, "nothing changed");
        let said: serde_json::Value =
            serde_json::from_str(&run(d.path(), Kind::Server, "tools", true).unwrap()).unwrap();
        assert_eq!(said["created"], "src/tools.rs");
        assert!(get("Cargo.toml").contains(
            "'.dependencies]\naio = { package = \"aio-rs\", git = \"https://github.com/riagentic/aio-rs\", tag = \"v"
        ));
        assert!(get("Cargo.toml").ends_with(", features = [\"server\"] }\n"), "natively: {}", get("Cargo.toml"));
        assert!(get("app/src/main.rs").contains("app::tools::http(aio::http::Http::default())"));
        assert!(run(d.path(), Kind::Cell, "../x", false).is_err());
    }
}

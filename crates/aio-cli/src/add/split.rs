//! The app's core crate (`core/`, `<app>-core`), made by the first `ax add
//! page` of an app whose library holds its cells. A page can't use the
//! library (the library depends on it), so the cells, their `client!` and
//! what they use move to a crate the library and every page depend on; the
//! library re-exports it whole (`pub use <app>_core::*;`), so every path into
//! it (`crate::prelude`, `app::SOURCE`, `crate::tally::Tally`) names the
//! same. Worked out on the text, tried on a copy (`cargo check`), and only
//! then written into the app.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;

/// The core crate's folder under the app's root.
pub const DIR: &str = "core";

const DOCS: &str = "//! The app's cells and their clients (`ax add page` moved them here from its
//! library): the library and every page depend on this crate, and the library
//! re-exports it whole, so each path into it reads as before. `ax add cell`
//! writes new cells here.
";

/// What making the core crate does: the files it writes (paths from the
/// app's root, `/`-separated) and the library's files it removes.
#[derive(Debug)]
pub struct Plan {
    pub package: String,
    /// What moves, as its first line says it (`pub struct Counter`, `mod tally;`).
    pub moved: Vec<String>,
    pub writes: Vec<(String, Vec<u8>)>,
    pub removes: Vec<String>,
}

impl Plan {
    /// The plan, said before it is made (`json`: as an object).
    pub fn said(&self, json: bool) -> serde_json::Value {
        let writes: Vec<&str> = self.writes.iter().map(|(f, _)| f.as_str()).collect();
        if json {
            return serde_json::json!({ "dir": DIR, "package": self.package, "moves": self.moved,
                "writes": writes, "removes": self.removes });
        }
        let krate = self.package.replace('-', "_");
        let mut out = vec![
            format!(
                "A page can't read the cells in the app's library (the library depends on its pages): they \
                 move to a crate of their own, {DIR}/ ({}), that the library and every page depend on.",
                self.package
            ),
            format!("Moves to {DIR}/src: {}", self.moved.join(", ")),
            format!("src/lib.rs re-exports it (pub use {krate}::*;): crate::prelude and every other path stay"),
        ];
        out.extend(writes.iter().map(|w| format!("  writes {w}")));
        out.extend(self.removes.iter().map(|r| format!("  removes {r}")));
        out.push("Tried on a copy first (cargo check): nothing changes unless it builds.".into());
        serde_json::Value::String(out.join("\n"))
    }
}

/// The package of the core crate of the app at `root`, if it has one.
pub fn existing(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(DIR).join("Cargo.toml")).ok()?;
    let t: toml::Table = text.parse().ok()?;
    Some(t.get("package")?.get("name")?.as_str()?.to_string())
}

/// One top-level item of the library.
struct Top {
    /// Its lines (0-based, inclusive), the plain comments just above it too.
    lines: (usize, usize),
    name: Option<String>,
    /// An impl's trait and type; a `use`'s names (`*`: a glob).
    head: BTreeSet<String>,
    /// Every name its tokens hold (a `mod x;`'s files' too).
    uses: BTreeSet<String>,
    seed: bool,
    kind: Kind,
    /// Where it becomes `pub`: line, from, to (chars), the text.
    public: Option<(usize, usize, usize, &'static str)>,
}

#[derive(PartialEq)]
enum Kind {
    Item,
    Impl,
    /// A `use`, by its first segment.
    Use(String),
    /// A `mod x;`: its files.
    File(Vec<String>),
    /// Never moved: views, tests, the browser build's start.
    Stays,
}

fn last_is(path: &syn::Path, name: &str) -> bool {
    path.segments.last().is_some_and(|s| s.ident == name)
}

fn has_attr(attrs: &[syn::Attribute], name: &str) -> bool {
    attrs.iter().any(|a| last_is(a.path(), name))
}

/// Are these the attributes of test code (`#[cfg(test)]`)?
fn is_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("cfg") && quote::ToTokens::to_token_stream(a).to_string().contains("test"))
}

/// Does `i` make a cell or its client (a module: in it, its tests aside)?
fn is_cell(i: &syn::Item) -> bool {
    match i {
        syn::Item::Struct(s) => has_attr(&s.attrs, "cell"),
        syn::Item::Enum(e) => has_attr(&e.attrs, "cell"),
        syn::Item::Impl(i) => has_attr(&i.attrs, "methods"),
        syn::Item::Macro(m) => last_is(&m.mac.path, "client"),
        syn::Item::Mod(m) => {
            !is_test(&m.attrs) && m.content.as_ref().is_some_and(|(_, items)| items.iter().any(is_cell))
        }
        _ => false,
    }
}

/// The names `ts` uses that may be the library's: `b` in `crate::b`, not
/// in `aio::b` (another crate's).
fn idents(ts: proc_macro2::TokenStream, out: &mut BTreeSet<String>) {
    use proc_macro2::TokenTree as T;
    let tokens: Vec<T> = ts.into_iter().collect();
    let colon = |t: &T| matches!(t, T::Punct(p) if p.as_char() == ':');
    for (i, t) in tokens.iter().enumerate() {
        match t {
            T::Group(g) => idents(g.stream(), out),
            T::Ident(id) => {
                let after = i >= 2 && colon(&tokens[i - 1]) && colon(&tokens[i - 2]);
                let ours = |t: &T| matches!(t, T::Ident(r) if r == "crate" || r == "self" || r == "super");
                if !after || (i >= 3 && ours(&tokens[i - 3])) {
                    out.insert(id.to_string());
                }
            }
            _ => {}
        }
    }
}

fn use_names(t: &syn::UseTree, out: &mut BTreeSet<String>) {
    match t {
        syn::UseTree::Path(p) => use_names(&p.tree, out),
        syn::UseTree::Name(n) => _ = out.insert(n.ident.to_string()),
        syn::UseTree::Rename(r) => _ = out.insert(r.rename.to_string()),
        syn::UseTree::Glob(_) => _ = out.insert("*".into()),
        syn::UseTree::Group(g) => g.items.iter().for_each(|t| use_names(t, out)),
    }
}

/// The files under `dir` (paths from `root`, `/`-separated), sorted.
fn files_under(root: &Path, dir: &Path) -> Vec<String> {
    let mut out = vec![];
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            match e.file_type().is_ok_and(|t| t.is_dir()) {
                true => todo.push(e.path()),
                false => out.push(rel(root, &e.path())),
            }
        }
    }
    out.sort();
    out
}

fn rel(root: &Path, f: &Path) -> String {
    f.strip_prefix(root).unwrap_or(f).to_string_lossy().replace('\\', "/")
}

/// The library's top-level items (`lib`: src/lib.rs of the app at `root`).
fn tops(root: &Path, lib: &str) -> Result<Vec<Top>, String> {
    let file = syn::parse_file(lib).map_err(|e| format!("src/lib.rs: {e}"))?;
    let lines: Vec<&str> = lib.lines().collect();
    let mut out: Vec<Top> = vec![];
    for item in &file.items {
        let (start, end) = (item.span().start().line - 1, item.span().end().line - 1);
        let floor = out.last().map_or(0, |t| t.lines.1 + 1);
        let comments = lines[floor.min(start)..start].iter().rev().take_while(|l| {
            let l = l.trim_start();
            l.starts_with("//") && !l.starts_with("//!")
        });
        let from = start - comments.count();
        let mut uses = BTreeSet::new();
        idents(quote::ToTokens::to_token_stream(item), &mut uses);
        let (mut head, mut seed, mut kind) = (BTreeSet::new(), is_cell(item), Kind::Item);
        use syn::Item as I;
        let (name, vis, kw): (Option<&syn::Ident>, Option<&syn::Visibility>, Option<proc_macro2::Span>) = match item {
            I::Struct(s) => (Some(&s.ident), Some(&s.vis), Some(s.struct_token.span)),
            I::Enum(e) => (Some(&e.ident), Some(&e.vis), Some(e.enum_token.span)),
            I::Union(u) => (Some(&u.ident), Some(&u.vis), Some(u.union_token.span)),
            I::Fn(f) => (Some(&f.sig.ident), Some(&f.vis), Some(f.sig.span())),
            I::Const(c) => (Some(&c.ident), Some(&c.vis), Some(c.const_token.span)),
            I::Static(s) => (Some(&s.ident), Some(&s.vis), Some(s.static_token.span)),
            I::Type(t) => (Some(&t.ident), Some(&t.vis), Some(t.type_token.span)),
            I::Trait(t) => (Some(&t.ident), Some(&t.vis), Some(t.unsafety.map_or(t.trait_token.span, |u| u.span))),
            I::Mod(m) => (Some(&m.ident), Some(&m.vis), Some(m.mod_token.span)),
            I::Impl(i) => {
                kind = Kind::Impl;
                let trait_ = i.trait_.as_ref().map(|(_, p, _)| quote::ToTokens::to_token_stream(p));
                idents(trait_.unwrap_or_default(), &mut head);
                idents(quote::ToTokens::to_token_stream(&i.self_ty), &mut head);
                (None, None, None)
            }
            I::Use(u) => {
                use_names(&u.tree, &mut head);
                let first = match &u.tree {
                    syn::UseTree::Path(p) => p.ident.to_string(),
                    _ => String::new(),
                };
                kind = Kind::Use(first);
                (None, None, None)
            }
            _ => (None, None, None),
        };
        let attrs = match item {
            I::Fn(i) => &i.attrs[..],
            I::Mod(i) => &i.attrs,
            I::Struct(i) => &i.attrs,
            I::Enum(i) => &i.attrs,
            I::Impl(i) => &i.attrs,
            I::Const(i) => &i.attrs,
            I::Static(i) => &i.attrs,
            I::Type(i) => &i.attrs,
            I::Trait(i) => &i.attrs,
            I::Use(i) => &i.attrs,
            I::Macro(i) => &i.attrs,
            _ => &[],
        };
        let view = matches!(item, I::Fn(f) if has_attr(&f.attrs, "component") || has_attr(&f.attrs, "wasm_bindgen"));
        if view || is_test(attrs) || (matches!(item, I::Macro(_)) && !seed) {
            kind = Kind::Stays;
        }
        if let I::Mod(m) = item
            && m.content.is_none()
            && kind != Kind::Stays
        {
            let n = m.ident.to_string();
            let (flat, dir) = (root.join("src").join(format!("{n}.rs")), root.join("src").join(&n));
            let mut files: Vec<String> = flat.is_file().then(|| format!("src/{n}.rs")).into_iter().collect();
            files.extend(files_under(root, &dir));
            let main = files.iter().find(|f| **f == format!("src/{n}.rs") || **f == format!("src/{n}/mod.rs"));
            let text = main.and_then(|f| std::fs::read_to_string(root.join(f)).ok());
            let parsed = text.as_deref().map(syn::parse_file).transpose().map_err(|e| format!("src/{n}.rs: {e}"))?;
            seed = parsed.as_ref().is_some_and(|f| f.items.iter().any(is_cell));
            if seed && has_attr(&m.attrs, "path") {
                return Err(format!("src/lib.rs: mod {n} has a #[path]: move it to {DIR}/ yourself"));
            }
            for f in files.iter().filter(|f| f.ends_with(".rs")) {
                let text = std::fs::read_to_string(root.join(f)).map_err(|e| format!("{f}: {e}"))?;
                idents(text.parse().map_err(|e| format!("{f}: {e}"))?, &mut uses);
            }
            kind = Kind::File(files);
        }
        let at = |s: proc_macro2::Span| (s.start().line - 1, s.start().column);
        let public = match (vis, kw) {
            (Some(syn::Visibility::Inherited), Some(k)) => Some((at(k).0, at(k).1, at(k).1, "pub ")),
            (Some(syn::Visibility::Restricted(r)), _) if r.span().start().line == r.span().end().line => {
                Some((at(r.span()).0, at(r.span()).1, r.span().end().column, "pub"))
            }
            _ => None,
        };
        out.push(Top { lines: (from, end), name: name.map(|n| n.to_string()), head, uses, seed, kind, public });
    }
    Ok(out)
}

/// Which of `tops` move (pure): the cells and their clients, the impls on
/// what moves, and what any of it names, views and tests aside.
fn moving(tops: &[Top]) -> Vec<bool> {
    let mut moving: Vec<bool> = tops.iter().map(|t| t.seed && t.kind != Kind::Stays).collect();
    loop {
        let names: BTreeSet<&str> =
            tops.iter().zip(&moving).filter(|(_, m)| **m).filter_map(|(t, _)| t.name.as_deref()).collect();
        let used: BTreeSet<&str> = tops
            .iter()
            .zip(&moving)
            .filter(|(_, m)| **m)
            .flat_map(|(t, _)| t.uses.iter().map(String::as_str))
            .collect();
        let mut grew = false;
        for (t, m) in tops.iter().zip(moving.iter_mut()) {
            let pull = match &t.kind {
                Kind::Stays | Kind::Use(_) => false,
                Kind::Impl => t.head.iter().any(|h| names.contains(h.as_str())),
                _ => t.name.as_deref().is_some_and(|n| used.contains(n)),
            };
            if pull && !*m {
                *m = true;
                grew = true;
            }
        }
        if !grew {
            return moving;
        }
    }
}

/// `lines[from..=to]`, made `pub` where `public` says (pure).
fn text_of(lines: &[&str], t: &Top, public: bool) -> String {
    let mut out: Vec<String> = lines[t.lines.0..=t.lines.1].iter().map(|l| l.to_string()).collect();
    if let (true, Some((line, from, to, word))) = (public, t.public) {
        let l = &mut out[line - t.lines.0];
        let byte = |c: usize| l.char_indices().nth(c).map_or(l.len(), |(b, _)| b);
        let (a, b) = (byte(from), byte(to));
        l.replace_range(a..b, word);
    }
    out.join("\n")
}

/// `lines` without those cut (`true`), nor a blank line the cut left
/// beside another, nor blank lines last (pure): what is kept is as it was,
/// a blank line in a string in it too.
fn without(lines: &[(&str, bool)]) -> String {
    let (mut out, mut gap): (Vec<&str>, bool) = (vec![], false);
    for (l, cut) in lines {
        let blank = l.trim().is_empty();
        if *cut || (gap && blank && out.last().is_none_or(|p| p.trim().is_empty())) {
            gap |= *cut;
            continue;
        }
        gap &= blank;
        out.push(l);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n") + "\n"
}

/// `text` (a manifest) with no run of blank lines, none first or last (pure).
fn tidy(text: &str) -> String {
    let mut out: Vec<&str> = vec![];
    for l in text.lines() {
        if l.trim().is_empty() && out.last().is_none_or(|p| p.trim().is_empty() || p.ends_with('{')) {
            continue;
        }
        out.push(l);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n") + "\n"
}

/// The core crate for the app at `root` (package `package`, its library
/// `lib`, its manifest `manifest`), or None: its library holds no cell.
pub fn plan(root: &Path, package: &str, lib: &str, manifest: &str) -> Result<Option<Plan>, String> {
    let tops = tops(root, lib)?;
    let moving = moving(&tops);
    if !moving.iter().any(|m| *m) {
        return Ok(None);
    }
    if root.join(DIR).exists() {
        return Err(format!("{DIR}/ is there already, and no crate: the cells' crate goes there; move it aside"));
    }
    let lines: Vec<&str> = lib.lines().collect();
    // Two items on one line, one moving: the cut would split it.
    for (a, b) in tops.iter().zip(&moving).zip(tops.iter().zip(&moving).skip(1)) {
        if a.1 != b.1 && a.0.lines.1 >= b.0.lines.0 {
            return Err(format!("src/lib.rs:{}: two items on one line: cargo fmt it first", b.0.lines.0 + 1));
        }
    }
    let package = format!("{package}-core");
    let krate = package.replace('-', "_");
    let used: BTreeSet<&str> =
        tops.iter().zip(&moving).filter(|(_, m)| **m).flat_map(|(t, _)| t.uses.iter().map(String::as_str)).collect();
    let pages: BTreeSet<String> = pages(manifest).into_iter().map(|(k, _)| k).collect();
    // The library's imports the moved code may need (an unused one is dropped after the check).
    let wanted = |t: &Top| match &t.kind {
        Kind::Use(first) => {
            !["crate", "self", "super"].contains(&first.as_str())
                && !pages.contains(first)
                && t.head.iter().any(|h| h == "*" || used.contains(h.as_str()))
        }
        _ => false,
    };
    let (mut core, mut prev) = (format!("{DOCS}\n"), None::<usize>);
    let (mut moved, mut removes, mut writes) = (vec![], vec![], vec![]);
    for (t, m) in tops.iter().zip(&moving) {
        if !*m && !wanted(t) {
            continue;
        }
        if prev.is_some_and(|p| p + 1 < t.lines.0) {
            core.push('\n');
        }
        core.push_str(&text_of(&lines, t, *m));
        core.push('\n');
        prev = Some(t.lines.1);
        if !*m {
            continue;
        }
        let first = lines[t.lines.0..=t.lines.1]
            .iter()
            .map(|l| l.trim())
            .find(|l| !l.starts_with("#[") && !l.starts_with("//"));
        moved.push(first.unwrap_or_default().trim_end_matches(" {").to_string());
        if let Kind::File(files) = &t.kind {
            for f in files {
                let bytes = std::fs::read(root.join(f)).map_err(|e| format!("{f}: {e}"))?;
                writes.push((format!("{DIR}/{f}"), bytes));
                removes.push(f.clone());
            }
        }
    }
    // The library: what moved cut, the core re-exported before its first item that stays.
    let at = tops
        .iter()
        .zip(&moving)
        .find(|(t, m)| !**m && !matches!(t.kind, Kind::Use(_)))
        .map_or(lines.len(), |(t, _)| t.lines.0);
    let cut: BTreeSet<usize> =
        tops.iter().zip(&moving).filter(|(_, m)| **m).flat_map(|(t, _)| t.lines.0..=t.lines.1).collect();
    let (said, reexport) = (
        format!("// The cells are in {DIR}/ ({package}), which the pages use too: re-exported"),
        format!("pub use {krate}::*;"),
    );
    let mut block = vec![
        (said.as_str(), false),
        ("// whole, so each path to them (crate::prelude, app::SOURCE) is as it was.", false),
        (reexport.as_str(), false),
        ("", false),
    ];
    // A blank line before it, unless the line kept before it is one.
    if (0..at).rev().find(|i| !cut.contains(i)).is_some_and(|i| !lines[i].trim().is_empty()) {
        block.insert(0, ("", false));
    }
    let mut app: Vec<(&str, bool)> = lines.iter().enumerate().map(|(i, l)| (*l, cut.contains(&i))).collect();
    app.splice(at..at, block);
    writes.insert(0, (format!("{DIR}/src/lib.rs"), core.into_bytes()));
    writes.insert(0, (format!("{DIR}/Cargo.toml"), manifest_of(&package, manifest).into_bytes()));
    writes.push(("src/lib.rs".into(), without(&app).into_bytes()));
    Ok(Some(Plan { package, moved, writes, removes }))
}

/// The library's page crates in `manifest` (`pages/<name>`): their crate
/// names and folders (pure).
pub fn pages(manifest: &str) -> Vec<(String, String)> {
    let t: toml::Table = manifest.parse().unwrap_or_default();
    let deps = t.get("dependencies").and_then(|d| d.as_table()).cloned().unwrap_or_default();
    let path = |d: &toml::Value| d.get("path").and_then(|p| p.as_str()).map(String::from);
    deps.iter().filter_map(|(k, d)| Some((k.replace('-', "_"), path(d).filter(|p| p.starts_with("pages/"))?))).collect()
}

/// A page's `manifest` with the core crate `package` (`up` folders above
/// it) among its dependencies (pure).
pub fn with_core_dep(manifest: &str, package: &str, up: usize) -> String {
    let line = format!("{package} = {{ path = \"{}{DIR}\" }}", "../".repeat(up));
    match manifest.contains("[dependencies]\n") {
        true => manifest.replacen("[dependencies]\n", &format!("[dependencies]\n{line}\n"), 1),
        false => format!("{}\n\n[dependencies]\n{line}\n", manifest.trim_end()),
    }
}

/// The core crate's manifest (pure): the library's dependencies (each table
/// of them, with the comments above it), a folder deeper, its pages aside.
pub fn manifest_of(package: &str, manifest: &str) -> String {
    let t: toml::Table = manifest.parse().unwrap_or_default();
    let shared = match t.get("workspace").and_then(|w| w.get("package")).is_some() {
        true => "edition.workspace = true\npublish.workspace = true",
        false => "edition = \"2024\"\npublish = false",
    };
    let lines: Vec<&str> = manifest.lines().collect();
    let heads: Vec<usize> = (0..lines.len()).filter(|i| lines[*i].trim_start().starts_with('[')).collect();
    let mut out = format!(
        "# The app's cells (`ax add page` made this crate): its library and every\n\
         # page depend on it. Its dependencies are the library's.\n[package]\nname = \"{package}\"\n\
         version = \"0.1.0\"\n{shared}\n"
    );
    for (n, h) in heads.iter().enumerate() {
        let head = lines[*h].trim();
        let deps = head.ends_with("dependencies]") || head.contains("dependencies.");
        if !deps || head.contains("build-dependencies") || head.starts_with("[workspace") {
            continue;
        }
        let from = h - lines[..*h].iter().rev().take_while(|l| l.trim_start().starts_with('#')).count();
        let to = heads.get(n + 1).map_or(lines.len(), |next| {
            next - lines[..*next].iter().rev().take_while(|l| l.trim_start().starts_with('#')).count()
        });
        out.push('\n');
        for l in &lines[from..to] {
            if !l.contains("path = \"pages/") {
                out.push_str(&crate::add::deeper(l, 1));
                out.push('\n');
            }
        }
    }
    tidy(&out)
}

/// What a page shows of a cell: its prelude's path and a field of it.
#[derive(Debug, PartialEq)]
pub struct Reading {
    pub prelude: String,
    /// The field, and whether it is a list (shown by its length).
    pub field: Option<(String, bool)>,
}

/// The first `client!` of core crate `krate` (its root's, else a module's;
/// `read` gives `src/<m>.rs` of it), as a page reads it (pure).
pub fn reading(krate: &str, lib: &str, read: &dyn Fn(&str) -> Option<String>) -> Option<Reading> {
    fn client(items: &[syn::Item]) -> Option<String> {
        items.iter().find_map(|i| match i {
            syn::Item::Macro(m) if last_is(&m.mac.path, "client") => {
                let cell = m.mac.tokens.clone().into_iter().take_while(|t| t.to_string() != ",");
                cell.filter_map(|t| matches!(t, proc_macro2::TokenTree::Ident(_)).then(|| t.to_string())).last()
            }
            _ => None,
        })
    }
    let file = syn::parse_file(lib).ok()?;
    let found = client(&file.items).map(|c| (String::new(), c, file.items.clone())).or_else(|| {
        file.items.iter().find_map(|i| match i {
            syn::Item::Mod(m) if m.content.is_none() => {
                let n = m.ident.to_string();
                let f = read(&format!("{n}.rs")).or_else(|| read(&format!("{n}/mod.rs")))?;
                let items = syn::parse_file(&f).ok()?.items;
                Some((format!("{n}::"), client(&items)?, items))
            }
            _ => None,
        })
    })?;
    let (module, cell, items) = found;
    const SHOWN: &[&str] = &[
        "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "bool",
        "char", "String",
    ];
    let field = items.iter().find_map(|i| match i {
        syn::Item::Struct(s) if s.ident == cell => s.fields.iter().find_map(|f| {
            let name = f.ident.as_ref()?.to_string();
            let syn::Type::Path(p) = &f.ty else { return None };
            let last = p.path.segments.last()?.ident.to_string();
            let list = last == "Vec";
            (!name.starts_with("r#") && (list || SHOWN.contains(&last.as_str()))).then_some((name, list))
        }),
        _ => None,
    });
    Some(Reading { prelude: format!("{krate}::{module}prelude"), field })
}

/// A copy of the app at `root` beside it (the same depth: its relative
/// paths hold), its build output aside; removed when dropped.
pub struct Trial(pub PathBuf);

impl Trial {
    pub fn of(root: &Path) -> Result<Trial, String> {
        let (parent, name) = (root.parent().ok_or("the app is at /")?, root.file_name().unwrap_or_default());
        let to = parent.join(format!(".{}.ax-core-{}", name.to_string_lossy(), std::process::id()));
        let copy = Trial(to.clone());
        let mut todo = vec![root.to_path_buf()];
        while let Some(d) = todo.pop() {
            let into = to.join(d.strip_prefix(root).unwrap_or(&d));
            std::fs::create_dir_all(&into).map_err(|e| format!("{}: {e}", into.display()))?;
            for e in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?.flatten() {
                let n = e.file_name();
                if d == root && [".git", "target", "dist", "node_modules", ".data"].iter().any(|s| n == *s) {
                    continue;
                }
                match e.file_type().is_ok_and(|t| t.is_dir()) {
                    true => todo.push(e.path()),
                    false => {
                        _ = std::fs::copy(e.path(), into.join(&n)).map_err(|e| format!("{}: {e}", into.display()))?
                    }
                }
            }
        }
        Ok(copy)
    }
}

impl Drop for Trial {
    fn drop(&mut self) {
        _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `cargo check` of every crate and target of the workspace at `dir`:
/// Err, its first errors; Ok, the lines (from 1) of `file` (a path from
/// `dir`) whose one-line `use` it found unused whole.
pub fn check(dir: &Path, file: &str) -> Result<Vec<usize>, String> {
    let out = std::process::Command::new("cargo")
        .args(["check", "--workspace", "--all-targets", "--message-format=json"])
        .current_dir(dir)
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| format!("cargo: {e}"))?;
    let (mut errors, mut unused) = (vec![], BTreeSet::new());
    for m in
        String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
    {
        let m = &m["message"];
        if m["level"] == "error" {
            errors.push(m["rendered"].as_str().unwrap_or_default().trim_end().to_string());
        }
        let spans = m["spans"].as_array().map_or(&[][..], Vec::as_slice);
        let [s] = spans else { continue };
        let name = s["file_name"].as_str().unwrap_or_default();
        if m["code"]["code"] != "unused_imports"
            || (Path::new(name) != Path::new(file) && Path::new(name) != dir.join(file))
        {
            continue;
        }
        let t = &s["text"][0];
        let (line, from, to) =
            (t["text"].as_str().unwrap_or_default(), t["highlight_start"].as_u64(), t["highlight_end"].as_u64());
        let shown: Option<String> =
            from.zip(to).map(|(a, b)| line.chars().skip(a as usize - 1).take((b - a) as usize).collect());
        // The whole of it (a glob's path is shown without its `::*`).
        if shown.is_some_and(|u| [format!("use {u};"), format!("use {u}::*;")].contains(&line.trim().to_string())) {
            unused.extend(s["line_start"].as_u64().map(|l| l as usize));
        }
    }
    match out.status.success() {
        true => Ok(unused.into_iter().collect()),
        false if errors.is_empty() => Err(String::from_utf8_lossy(&out.stdout).chars().take(2000).collect()),
        false => Err(errors.into_iter().take(3).collect::<Vec<_>>().join("\n\n")),
    }
}

/// `text` without its lines `cut` (from 1), as [`without`] (pure).
pub fn edited(text: &str, cut: &[usize]) -> String {
    let lines: Vec<(&str, bool)> = text.lines().enumerate().map(|(i, l)| (l, cut.contains(&(i + 1)))).collect();
    without(&lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new app of `template` at `d`, as `ax create` writes it.
    fn made(d: &Path, template: &str) -> (String, String) {
        let n = crate::create::New { template, ..crate::create::New::of("notes") };
        for (f, text) in crate::create::files(&n).unwrap() {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let read = |f: &str| std::fs::read_to_string(d.join(f)).unwrap();
        (read("src/lib.rs"), read("Cargo.toml"))
    }

    fn file<'a>(p: &'a Plan, f: &str) -> &'a str {
        std::str::from_utf8(&p.writes.iter().find(|(w, _)| w == f).unwrap().1).unwrap()
    }

    #[test]
    fn the_cells_and_what_they_use_move_the_views_stay() {
        let d = tempfile::tempdir().unwrap();
        let (lib, manifest) = made(d.path(), "app");
        let p = plan(d.path(), "notes", &lib, &manifest).unwrap().unwrap();
        assert_eq!(p.package, "notes-core");
        assert_eq!(p.moved, ["pub struct Counter", "impl Counter", "aio::client!(Counter, counter);"]);
        let (core, app) = (file(&p, "core/src/lib.rs"), file(&p, "src/lib.rs"));
        assert!(core.starts_with(DOCS) && core.contains("\n#[aio::cell(persist, coalesce, id = \"counter\")]\n"));
        assert!(core.contains("use aio::ui::prelude::*;\n"), "kept until the check finds it unused: {core}");
        assert!(!core.contains("fn App") && !core.contains("mod tests"), "{core}");
        assert!(!app.contains("pub struct Counter") && !app.contains("client!(Counter"), "{app}");
        assert!(app.contains("use aio::ui::prelude::*;\n\n// The cells are in core/ (notes-core)"), "{app}");
        assert!(app.contains("\npub use notes_core::*;\n\n/// The window:"), "{app}");
        for kept in ["pub fn App()", "mod controls {", "pub fn web()", "mod tests {"] {
            assert!(app.contains(kept), "{kept}: {app}");
        }
        assert!(!app.contains("\n\n\n") && !core.contains("\n\n\n"));
        assert!(p.removes.is_empty());

        // The todo list: its item type too, not the window's filter.
        let d = tempfile::tempdir().unwrap();
        let (lib, manifest) = made(d.path(), "todo");
        let p = plan(d.path(), "notes", &lib, &manifest).unwrap().unwrap();
        assert_eq!(p.moved, ["pub struct Todo", "pub struct Todos", "impl Todos", "aio::client!(Todos, todos);"]);
        // The canvas: the pure function its method calls, and its Default.
        let d = tempfile::tempdir().unwrap();
        let (lib, manifest) = made(d.path(), "canvas");
        let p = plan(d.path(), "notes", &lib, &manifest).unwrap().unwrap();
        let moved =
            ["pub struct Ball", "pub struct World", "impl Default for World", "pub fn step(w: &World) -> Vec<Ball>"];
        assert_eq!(p.moved[..4], moved, "{:?}", p.moved);
        assert!(!p.moved.iter().any(|m| m.contains("draw")), "{:?}", p.moved);
    }

    #[test]
    fn a_cells_module_moves_with_its_files_and_private_items_turn_pub() {
        let d = tempfile::tempdir().unwrap();
        let (_, manifest) = made(d.path(), "app");
        crate::add::run(d.path(), crate::add::Kind::Cell, "tally", false).unwrap();
        // Server functions stay: the cell in their tests is no cell of the app's.
        crate::add::run(d.path(), crate::add::Kind::Server, "tools", false).unwrap();
        std::fs::create_dir_all(d.path().join("src/tally")).unwrap();
        std::fs::write(d.path().join("src/tally/rows.json"), "[]").unwrap();
        // A helper the cell calls, private, and a view module nothing moved names.
        let lib = std::fs::read_to_string(d.path().join("src/lib.rs")).unwrap().replace(
            "aio::client!(Counter, counter);",
            "aio::client!(Counter, counter);\n\n// Twice.\npub(crate) fn twice(n: i64) -> i64 {\n    n * 2\n}\n\nconst UNUSED: i64 = 1;",
        );
        let lib = lib.replace("        self.count += by;", "        self.count += twice(by) / 2;");
        let p = plan(d.path(), "notes", &lib, &manifest).unwrap().unwrap();
        assert_eq!(p.moved[0], "pub mod tally;");
        assert!(p.moved.contains(&"pub(crate) fn twice(n: i64) -> i64".to_string()), "{:?}", p.moved);
        assert_eq!(p.removes, ["src/tally.rs", "src/tally/rows.json"]);
        assert!(p.writes.iter().any(|(f, b)| f == "core/src/tally/rows.json" && b == b"[]"));
        let core = file(&p, "core/src/lib.rs");
        assert!(core.contains("// Twice.\npub fn twice(n: i64) -> i64 {"), "{core}");
        assert!(!core.contains("UNUSED"));
        let app = file(&p, "src/lib.rs");
        assert!(app.contains("const UNUSED") && !app.contains("pub mod tally;") && !app.contains("fn twice"), "{app}");
        assert!(app.contains("pub mod tools;") && !p.moved.iter().any(|m| m.contains("tools")), "{app}");
        // The new page reads the first client! there is (the root's).
        let read = |f: &str| {
            p.writes
                .iter()
                .find(|(w, _)| *w == format!("core/src/{f}"))
                .map(|(_, b)| String::from_utf8(b.clone()).unwrap())
        };
        let r = reading("notes_core", core, &read).unwrap();
        assert_eq!(r, Reading { prelude: "notes_core::prelude".into(), field: Some(("count".into(), false)) });
        // A module's when the root has none.
        let r = reading("notes_core", "pub mod tally;\n", &read).unwrap();
        assert_eq!(r.prelude, "notes_core::tally::prelude");
        assert_eq!(reading("notes_core", "pub fn x() {}\n", &read), None);
    }

    #[test]
    fn a_library_without_cells_makes_no_core() {
        let d = tempfile::tempdir().unwrap();
        let lib = "//! A tool.\nuse std::fmt;\n\npub fn x() {}\n";
        assert!(plan(d.path(), "notes", lib, "").unwrap().is_none());
        assert!(plan(d.path(), "notes", "fn (", "").unwrap_err().contains("src/lib.rs"));
    }

    #[test]
    fn the_core_crate_takes_the_librarys_dependencies_a_folder_deeper() {
        let m = "[workspace]\nmembers = [\".\", \"app\", \"pages/a\"]\n\n[workspace.dependencies]\nx = \"1\"\n\n\
                 [package]\nname = \"notes\"\n\n# From a checkout.\n[dependencies]\n\
                 aio = { package = \"aio-rs\", path = \"../aio/crates/aio\", features = [\"kit\"] }\n\
                 a = { package = \"notes-a\", path = \"pages/a\" }\nlocal = { path = \"/opt/local\" }\n\n\
                 [dev-dependencies]\naio = { package = \"aio-rs\", path = \"../aio/crates/aio\" }\n\n\
                 [dependencies.ureq]\nversion = \"3\"\n\n[profile.dev]\ndebug = 0\n";
        let core = manifest_of("notes-core", m);
        assert_eq!(
            core,
            "# The app's cells (`ax add page` made this crate): its library and every\n\
             # page depend on it. Its dependencies are the library's.\n[package]\nname = \"notes-core\"\n\
             version = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n# From a checkout.\n[dependencies]\n\
             aio = { package = \"aio-rs\", path = \"../../aio/crates/aio\", features = [\"kit\"] }\n\
             local = { path = \"/opt/local\" }\n\n[dev-dependencies]\n\
             aio = { package = \"aio-rs\", path = \"../../aio/crates/aio\" }\n\n[dependencies.ureq]\nversion = \"3\"\n"
        );
        assert_eq!(pages(m), [("a".to_string(), "pages/a".to_string())]);
        let page = "[package]\nname = \"notes-a\"\n\n[dependencies]\naio = { path = \"x\" }\n";
        assert!(
            with_core_dep(page, "notes-core", 2)
                .contains("[dependencies]\nnotes-core = { path = \"../../core\" }\naio")
        );
    }

    #[test]
    fn unused_imports_are_cut_and_the_blank_lines_left_tidied() {
        let text = "//! A.\n\nuse a::b;\nuse c::d;\n\npub fn x() {}\n";
        assert_eq!(edited(text, &[3]), "//! A.\n\nuse c::d;\n\npub fn x() {}\n");
        assert_eq!(edited(text, &[3, 4]), "//! A.\n\npub fn x() {}\n");
        // Blank lines elsewhere (in a string) are kept.
        let text = "use a::b;\n\npub const S: &str = \"x\n\n\ny\";\n";
        assert_eq!(edited(text, &[1]), "pub const S: &str = \"x\n\n\ny\";\n");
    }
}

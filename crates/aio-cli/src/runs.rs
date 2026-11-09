//! `ax where <app> [item]` (aio's `am where`): where an item's code runs,
//! answered from the app's crate graph and the `cfg`s on the way to it,
//! never guessed. The crate graph says which builds compile a crate (the
//! launcher: the app's process only; the library the web build binds, and
//! the crates it uses: the page too); each `cfg` from the crate's root
//! through its modules to the item is evaluated for both builds; what the
//! item is (a component, a method, a `#[client]` method) and the app's
//! mode say the rest.
use quote::ToTokens;
use std::path::{Path, PathBuf};
use syn::punctuated::Punctuated;
use syn::{Attribute, Item, Meta, Token};

/// The full map, as the verdicts and the docs both say it.
pub const MAP: &str = "docs/basics/architecture.md#where-each-part-of-the-code-runs";

/// Is a `cfg` true for a build: yes, no, or it depends (a feature, the OS).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tri {
    Yes,
    No,
    Maybe,
}

/// `cfg(meta)` for the page's build (`wasm`) or the app's process (pure).
fn eval(meta: &Meta, wasm: bool) -> Tri {
    use Tri::*;
    let list = |m: &syn::MetaList| -> Vec<Tri> {
        m.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .map(|ms| ms.iter().map(|m| eval(m, wasm)).collect())
            .unwrap_or_else(|_| vec![Maybe])
    };
    match meta {
        Meta::List(m) if m.path.is_ident("all") => {
            let v = list(m);
            if v.contains(&No) {
                No
            } else if v.iter().all(|t| *t == Yes) {
                Yes
            } else {
                Maybe
            }
        }
        Meta::List(m) if m.path.is_ident("any") => {
            let v = list(m);
            if v.contains(&Yes) {
                Yes
            } else if v.iter().all(|t| *t == No) {
                No
            } else {
                Maybe
            }
        }
        Meta::List(m) if m.path.is_ident("not") => match list(m).as_slice() {
            [Yes] => No,
            [No] => Yes,
            _ => Maybe,
        },
        // `test` is no build's: such an item is the tests'.
        Meta::Path(p) if p.is_ident("test") => No,
        Meta::Path(p) if wasm && (p.is_ident("unix") || p.is_ident("windows")) => No,
        Meta::NameValue(nv) => {
            let key = nv.path.get_ident().map(|i| i.to_string()).unwrap_or_default();
            let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(v), .. }) = &nv.value else { return Maybe };
            let v = v.value();
            let page = |yes: bool| if yes { Yes } else { No };
            match (key.as_str(), wasm) {
                ("target_arch", true) => page(v == "wasm32"),
                ("target_family", true) => page(v == "wasm"),
                ("target_os", true) => page(v == "unknown"),
                ("target_arch", false) if v.starts_with("wasm") => No,
                ("target_family", false) if v == "wasm" => No,
                ("target_os", false) if v == "unknown" => No,
                _ => Maybe,
            }
        }
        _ => Maybe,
    }
}

/// The `cfg`s among `attrs`.
fn cfgs(attrs: &[Attribute]) -> Vec<Meta> {
    attrs.iter().filter(|a| a.path().is_ident("cfg")).filter_map(|a| a.parse_args::<Meta>().ok()).collect()
}

fn has(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|a| a.path().segments.last().is_some_and(|s| s.ident == name))
}

/// What an item is, for where it runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// A component, or a fn that returns a `View`.
    View,
    /// A method of a `#[methods]` impl (`client`: `#[client]`, a synced copy predicts it).
    Method { client: bool, slow: bool },
    /// Anything else: a type, a helper, a constant.
    Other,
}

/// One item of the app's code.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// `Type::method`, `module::fn`, as the crate names it from its root.
    pub name: String,
    pub file: PathBuf,
    pub line: usize,
    pub kind: Kind,
    /// The `cfg`s from the crate's root to the item, as written.
    pub cfg: Vec<String>,
    /// Compiled into the app's process, the page's build.
    native: Tri,
    wasm: Tri,
    /// The modules from the crate's root (its files), to the item's.
    pub chain: Vec<String>,
}

/// Every item of the crate whose root is `root`, read with `read` (pure but for `read`).
pub fn walk(root: &Path, read: &dyn Fn(&Path) -> Result<String, String>) -> Result<Vec<Found>, String> {
    let file = syn::parse_file(&read(root)?).map_err(|e| format!("{}: {e}", root.display()))?;
    let mods = root.parent().unwrap_or(Path::new("")).to_path_buf();
    let mut out = vec![];
    let at = At { file: root.to_path_buf(), mods, path: vec![], cfg: cfgs(&file.attrs), chain: vec![show(root)] };
    items(&file.items, &at, read, &mut out)?;
    Ok(out)
}

/// A path as `ax where` prints it: `/` between parts on every OS.
fn show(p: &Path) -> String {
    p.display().to_string().replace(std::path::MAIN_SEPARATOR, "/")
}

/// Where the walk is: the file, the folder its `mod x;`s are in, the module path, the cfgs so far.
#[derive(Clone)]
struct At {
    file: PathBuf,
    mods: PathBuf,
    path: Vec<String>,
    cfg: Vec<Meta>,
    chain: Vec<String>,
}

impl At {
    fn found(&self, name: &str, attrs: &[Attribute], span: proc_macro2::Span, kind: Kind) -> Found {
        let all: Vec<Meta> = self.cfg.iter().cloned().chain(cfgs(attrs)).collect();
        let tri = |wasm| {
            let v: Vec<Tri> = all.iter().map(|m| eval(m, wasm)).collect();
            if v.contains(&Tri::No) {
                Tri::No
            } else if v.iter().all(|t| *t == Tri::Yes) {
                Tri::Yes
            } else {
                Tri::Maybe
            }
        };
        Found {
            name: self.path.iter().cloned().chain([name.to_string()]).collect::<Vec<_>>().join("::"),
            file: self.file.clone(),
            line: span.start().line,
            kind,
            cfg: all
                .iter()
                .map(|m| format!("cfg({})", m.to_token_stream()).replace(" (", "(").replace(" ,", ","))
                .collect(),
            native: tri(false),
            wasm: tri(true),
            chain: self.chain.clone(),
        }
    }
}

fn items(
    list: &[Item],
    at: &At,
    read: &dyn Fn(&Path) -> Result<String, String>,
    out: &mut Vec<Found>,
) -> Result<(), String> {
    for item in list {
        match item {
            Item::Fn(f) => {
                let view = has(&f.attrs, "component")
                    || matches!(&f.sig.output, syn::ReturnType::Type(_, t)
                        if matches!(&**t, syn::Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "View")));
                let kind = if view { Kind::View } else { Kind::Other };
                out.push(at.found(&f.sig.ident.to_string(), &f.attrs, f.sig.ident.span(), kind));
            }
            Item::Struct(s) => out.push(at.found(&s.ident.to_string(), &s.attrs, s.ident.span(), Kind::Other)),
            Item::Enum(e) => out.push(at.found(&e.ident.to_string(), &e.attrs, e.ident.span(), Kind::Other)),
            Item::Trait(t) => out.push(at.found(&t.ident.to_string(), &t.attrs, t.ident.span(), Kind::Other)),
            Item::Impl(i) => {
                let ty = i.self_ty.to_token_stream().to_string().replace(' ', "");
                let methods = has(&i.attrs, "methods");
                let mut inner = at.clone();
                inner.cfg.extend(cfgs(&i.attrs));
                inner.path.push(ty);
                for f in i.items.iter().filter_map(|x| if let syn::ImplItem::Fn(f) = x { Some(f) } else { None }) {
                    let kind = match methods {
                        true => Kind::Method { client: has(&f.attrs, "client"), slow: f.sig.asyncness.is_some() },
                        false => Kind::Other,
                    };
                    out.push(inner.found(&f.sig.ident.to_string(), &f.attrs, f.sig.ident.span(), kind));
                }
            }
            Item::Mod(m) => {
                let name = m.ident.to_string();
                let mut inner = at.clone();
                inner.cfg.extend(cfgs(&m.attrs));
                inner.path.push(name.clone());
                inner.mods = at.mods.join(&name);
                let Some((_, body)) = &m.content else {
                    let path = m.attrs.iter().find(|a| a.path().is_ident("path")).and_then(|a| match &a.meta {
                        Meta::NameValue(nv) => match &nv.value {
                            syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => {
                                Some(at.file.parent().unwrap_or(Path::new("")).join(s.value()))
                            }
                            _ => None,
                        },
                        _ => None,
                    });
                    let file = path
                        .or_else(|| {
                            [at.mods.join(format!("{name}.rs")), at.mods.join(&name).join("mod.rs")]
                                .into_iter()
                                .find(|p| read(p).is_ok())
                        })
                        .ok_or_else(|| format!("{}: mod {name}: no {name}.rs or {name}/mod.rs", show(&at.file)))?;
                    let parsed = syn::parse_file(&read(&file)?).map_err(|e| format!("{}: {e}", file.display()))?;
                    inner.cfg.extend(cfgs(&parsed.attrs));
                    if file.file_name().is_some_and(|f| f == "mod.rs") {
                        inner.mods = file.parent().unwrap_or(Path::new("")).to_path_buf();
                    }
                    inner.file = file.clone();
                    inner.chain.push(show(&file));
                    out.push(at.found(&name, &m.attrs, m.ident.span(), Kind::Other));
                    items(&parsed.items, &inner, read, out)?;
                    continue;
                };
                items(body, &inner, read, out)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// What an app's crates are: each one's root files and whether the page's build compiles it.
pub struct Crate {
    pub name: String,
    /// The crate's root files (its lib, its bins), by kind.
    pub roots: Vec<PathBuf>,
    /// The launcher (a binary's package): the app's process only.
    pub launcher: bool,
    /// The web build compiles it (the library it binds, and the crates that library uses).
    pub page: bool,
}

/// The verdict for `f` in crate `c` of an app in `mode` (`served`: its cells
/// are served, mode `server` or `client-server`), and the sentence that says it (pure).
pub fn verdict(f: &Found, c: &Crate, served: bool) -> (&'static str, String) {
    let native = f.native != Tri::No;
    let page = c.page && f.wasm != Tri::No;
    // `test` as a word of a cfg (not inside a string, not negated).
    let tests = f.cfg.iter().any(|c| {
        !c.contains("not(")
            && c.split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '"')).any(|w| w == "test")
    });
    let builds = match (native, page) {
        (_, _) if tests && !native && !page => {
            return ("tests-only", "only in the app's tests (cfg(test)): no build has it".into());
        }
        (false, false) => return ("never", "in no build: its cfgs rule out the app's process and the page".into()),
        (false, true) => {
            return (
                "wasm-only",
                "only in the web build: the browser's page runs it as wasm; the app's process never has it".into(),
            );
        }
        (true, true) => "the app's process (desktop, Android, server) and the web build's page (wasm)",
        (true, false) => "the app's process only (desktop, Android, server); never the page",
    };
    if c.launcher {
        return (
            "native-only",
            "in the launcher: the app's process, once at start (flags, sources, the window or the server)".into(),
        );
    }
    match f.kind {
        Kind::View if page => (
            "window/page",
            format!("a view: it draws the window in the app's process, and the page as wasm in a web build ({builds})"),
        ),
        Kind::View => ("window", format!("a view: it draws the window, in the app's process ({builds})")),
        Kind::Method { client, slow } => {
            let split = if slow {
                "; async: split at each await, the awaited futures are effects on the server's runtime"
            } else {
                ""
            };
            match (served, client) {
                (true, true) => (
                    "both",
                    format!(
                        "a #[client] method: it runs on the server, and on each synced copy as a prediction (a client window, a browser tab){split}"
                    ),
                ),
                (true, false) => (
                    "server",
                    format!(
                        "a method: it runs on the server, where the cell lives; not #[client], so no copy runs it{split}"
                    ),
                ),
                (false, _) if page => (
                    "both",
                    format!(
                        "a method: it runs where the cell lives: the app's process, or the page in a web build (its cells run there){split}"
                    ),
                ),
                (false, _) => {
                    ("native-only", format!("a method: it runs where the cell lives, the app's process{split}"))
                }
            }
        }
        Kind::Other if page => ("both", format!("compiled into {builds}: what it calls must exist in both")),
        Kind::Other => ("native-only", format!("compiled into {builds}")),
    }
}

/// What follows from a verdict (pure): the rules people trip on.
pub fn rules(f: &Found, page: bool) -> Vec<&'static str> {
    let mut r = match f.kind {
        Kind::View => vec![
            "a read subscribes only while it renders (a hole, a memo); a read in a handler doesn't",
            "a handler calls methods (act!), sets signals, spawns; a refusal is None",
        ],
        Kind::Method { .. } => vec![
            "it may run twice (a copy's prediction, then the server): no I/O, no clock, no randomness but cx",
            "I/O goes in an effect, an awaited future, a route or a server function",
        ],
        Kind::Other => vec![],
    };
    if page {
        r.push("in the page: no store history, files, blobs, SQL, server or anything native; keep that behind #[cfg(not(target_arch = \"wasm32\"))]");
    }
    r
}

/// The items of `all` that `ask` names: a file (`src/x.rs`), or an item by
/// its name, its `Type::method` or its module path's tail (pure).
pub fn matching<'a>(all: &'a [(usize, Found)], ask: &str) -> Vec<&'a (usize, Found)> {
    let file = ask.ends_with(".rs");
    all.iter()
        .filter(|(_, f)| match file {
            true => f.file.ends_with(ask),
            false => f.name == ask || f.name.ends_with(&format!("::{ask}")),
        })
        .collect()
}

/// `ax where <app> [item] [--json]`.
pub fn run(app: &str, ask: Option<&str>, json: bool) -> Result<String, String> {
    let meta = crate::dev::metadata()?;
    let pkg = crate::dev::locate(&meta, app)?;
    let (_, cfg) = crate::build::app_of(&pkg)?;
    let served =
        matches!(cfg.app.mode, aio::config::Mode::Server | aio::config::Mode::ClientServer | aio::config::Mode::Client);
    let dir = crate::build::app_dir(&pkg)?;
    let crates = crates(&meta, &pkg.dirs)?;
    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let mut all = vec![];
    for (i, c) in crates.iter().enumerate() {
        for r in &c.roots {
            all.extend(walk(r, &read)?.into_iter().map(|f| (i, f)));
        }
    }
    let rel = |p: &Path| show(p.strip_prefix(&dir).unwrap_or(p));
    let shown: Vec<&(usize, Found)> = match ask {
        Some(a) => {
            let m = matching(&all, a);
            if m.is_empty() {
                return Err(format!(
                    "{app}: no item or file `{a}` in its crates (an item by name, Type::method, or a file like src/lib.rs)"
                ));
            }
            m
        }
        None => all.iter().collect(),
    };
    let graph = |c: &Crate| {
        let launcher = crates.iter().find(|c| c.launcher).map(|l| l.name.clone());
        launcher.into_iter().chain((!c.launcher).then(|| c.name.clone())).collect::<Vec<_>>().join(" → ")
    };
    if json {
        let rows: Vec<serde_json::Value> = shown
            .iter()
            .map(|(i, f)| {
                let c = &crates[*i];
                let (v, says) = verdict(f, c, served);
                serde_json::json!({
                    "item": f.name, "file": rel(&f.file), "line": f.line, "crate": c.name, "verdict": v,
                    "says": says, "cfg": f.cfg, "crates": graph(c),
                    "modules": f.chain.iter().map(|p| rel(Path::new(p))).collect::<Vec<_>>(),
                    "rules": rules(f, c.page && f.wasm != Tri::No),
                })
            })
            .collect();
        return Ok(serde_json::Value::Array(rows).to_string());
    }
    let one = ask.is_some_and(|a| !a.ends_with(".rs")) && shown.len() == 1;
    let text = match one {
        true => {
            let (i, f) = shown[0];
            let c = &crates[*i];
            let (v, says) = verdict(f, c, served);
            let mut lines = vec![
                format!("  item        {} ({}:{})", f.name, rel(&f.file), f.line),
                format!("  runs        {v}: {says}"),
                format!(
                    "  reached by  {}: {}",
                    graph(c),
                    f.chain.iter().map(|p| rel(Path::new(p))).collect::<Vec<_>>().join(" → ")
                ),
                format!("  cfg         {}", if f.cfg.is_empty() { "none".into() } else { f.cfg.join(", ") }),
            ];
            lines.extend(rules(f, c.page && f.wasm != Tri::No).into_iter().map(|r| format!("  · {r}")));
            lines.join("\n")
        }
        false => {
            let head: Vec<String> = crates
                .iter()
                .map(|c| {
                    let builds = match (c.launcher, c.page) {
                        (true, _) => "the launcher: the app's process only",
                        (false, true) => "the app's process, and the page (the web build binds it)",
                        (false, false) => "the app's process (no web build)",
                    };
                    format!("{} ({}): {builds}", c.name, c.roots.iter().map(|r| rel(r)).collect::<Vec<_>>().join(", "))
                })
                .collect();
            let rows = shown.iter().map(|(i, f)| {
                let (v, _) = verdict(f, &crates[*i], served);
                format!("  {v:<12} {}:{}  {}", rel(&f.file), f.line, f.name)
            });
            head.into_iter().chain(rows).collect::<Vec<_>>().join("\n")
        }
    };
    Ok(format!("{text}\n  the full map: {MAP}"))
}

/// The app's crates from `cargo metadata` (`dirs`: its packages, the launcher first).
fn crates(meta: &serde_json::Value, dirs: &[PathBuf]) -> Result<Vec<Crate>, String> {
    let pkgs = meta["packages"].as_array().ok_or("cargo metadata: no packages")?;
    let of =
        |d: &Path| pkgs.iter().find(|p| p["manifest_path"].as_str().and_then(|m| Path::new(m).parent()) == Some(d));
    // The page's crates: the library the web build binds, and its path dependencies.
    let mut page: Vec<PathBuf> = crate::web::web_lib(meta, dirs)
        .and_then(|(name, _)| pkgs.iter().find(|p| p["name"] == name.as_str()))
        .and_then(|p| Some(Path::new(p["manifest_path"].as_str()?).parent()?.to_path_buf()))
        .into_iter()
        .collect();
    let mut i = 0;
    while let Some(d) = page.get(i).cloned() {
        i += 1;
        for dep in of(&d).and_then(|p| p["dependencies"].as_array()).into_iter().flatten() {
            if let Some(path) =
                dep["path"].as_str().map(PathBuf::from).filter(|p| dirs.contains(p) && !page.contains(p))
            {
                page.push(path);
            }
        }
    }
    Ok(dirs
        .iter()
        .filter_map(|d| {
            let p = of(d)?;
            let targets = p["targets"].as_array()?;
            let kind =
                |t: &serde_json::Value, k: &str| t["kind"].as_array().is_some_and(|ks| ks.iter().any(|x| x == k));
            let code = ["lib", "rlib", "cdylib", "bin"];
            let roots = targets
                .iter()
                .filter(|t| code.iter().any(|k| kind(t, k)))
                .filter_map(|t| t["src_path"].as_str().map(PathBuf::from))
                .collect();
            Some(Crate {
                name: p["name"].as_str()?.to_string(),
                roots,
                launcher: targets.iter().any(|t| kind(t, "bin")),
                page: page.contains(d),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn app() -> HashMap<PathBuf, &'static str> {
        HashMap::from([
            (
                PathBuf::from("src/lib.rs"),
                r#"
#[aio::cell(persist)]
pub struct Notes { pub list: Vec<String> }

#[aio::methods]
impl Notes {
    #[client]
    pub fn add(&mut self, t: String) { self.list.push(t); }
    pub fn purge(&mut self) { self.list.clear(); }
}

#[component]
pub fn App() -> View { rsx! { p { "hi" } } }

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn web() {}

#[cfg(not(target_arch = "wasm32"))]
mod server;

mod ui { pub fn row() -> View { todo!() } }

#[cfg(test)]
mod tests { fn it() {} }
"#,
            ),
            (PathBuf::from("src/server.rs"), "pub fn export() {}\n#[cfg(feature = \"pg\")]\npub fn pg() {}\n"),
        ])
    }

    fn found() -> Vec<(usize, Found)> {
        let files = app();
        let read = |p: &Path| files.get(p).map(|s| s.to_string()).ok_or_else(|| format!("{}: none", p.display()));
        walk(Path::new("src/lib.rs"), &read).unwrap().into_iter().map(|f| (0, f)).collect()
    }

    fn says(all: &[(usize, Found)], ask: &str, page: bool, served: bool) -> &'static str {
        let c = Crate { name: "notes".into(), roots: vec![], launcher: false, page };
        let m = matching(all, ask);
        assert_eq!(m.len(), 1, "{ask}: {:?}", m.iter().map(|(_, f)| &f.name).collect::<Vec<_>>());
        verdict(&m[0].1, &c, served).0
    }

    #[test]
    fn each_item_runs_where_its_crate_cfgs_and_kind_say() {
        let all = found();
        // A web build, cells served by a server.
        assert_eq!(says(&all, "App", true, true), "window/page");
        assert_eq!(says(&all, "row", true, true), "window/page", "a fn returning a View");
        assert_eq!(says(&all, "Notes::add", true, true), "both", "#[client]: the server and each copy");
        assert_eq!(says(&all, "purge", true, true), "server");
        assert_eq!(says(&all, "Notes", true, true), "both");
        assert_eq!(says(&all, "web", true, true), "wasm-only");
        assert_eq!(says(&all, "server::export", true, true), "native-only", "its mod's cfg(not wasm32)");
        assert_eq!(says(&all, "tests::it", true, true), "tests-only");
        // No web build: a wasm32 item is in no build; standalone: methods in the process.
        assert_eq!(says(&all, "web", false, false), "never");
        assert_eq!(says(&all, "purge", false, false), "native-only");
        assert_eq!(says(&all, "purge", true, false), "both", "a standalone web build's cells run in the page");
        assert_eq!(says(&all, "App", false, false), "window");
        let pg = &matching(&all, "pg")[0].1;
        assert_eq!(pg.cfg, ["cfg(not(target_arch = \"wasm32\"))", "cfg(feature = \"pg\")"]);
        assert_eq!(pg.chain, ["src/lib.rs", "src/server.rs"]);
        assert_eq!((pg.file.as_path(), pg.line), (Path::new("src/server.rs"), 3));
        let launcher = Crate { name: "notes-app".into(), roots: vec![], launcher: true, page: false };
        assert_eq!(verdict(&matching(&all, "App")[0].1, &launcher, false).0, "native-only");
        assert_eq!(matching(&all, "server.rs").len(), 2, "a file: its items");
        assert!(rules(&matching(&all, "add")[0].1, true).iter().any(|r| r.contains("may run twice")));
    }

    #[test]
    fn a_cfg_is_evaluated_for_each_build() {
        let e = |s: &str, wasm| eval(&syn::parse_str::<Meta>(s).unwrap(), wasm);
        assert_eq!((e("target_arch = \"wasm32\"", true), e("target_arch = \"wasm32\"", false)), (Tri::Yes, Tri::No));
        assert_eq!(e("not(target_arch = \"wasm32\")", false), Tri::Yes);
        assert_eq!(e("all(unix, feature = \"x\")", true), Tri::No);
        assert_eq!(e("all(unix, feature = \"x\")", false), Tri::Maybe);
        assert_eq!(e("any(target_os = \"android\", target_family = \"wasm\")", true), Tri::Yes);
        assert_eq!(e("target_os = \"android\"", true), Tri::No);
        assert_eq!(e("test", false), Tri::No);
    }
}

//! `ax dev`'s template hot reload, for an app on air: a saved `.rs` file
//! whose only change since it was built is in the markup of its `rsx!`
//! calls (text, a static attribute, static elements: the holes the same)
//! updates the running window without a build. Each `rsx!` call is read
//! as the compiler reads it (air's own `rsx!` parser, compiled into `ax`);
//! the edited templates go to the app over its `ui` door (`Aio.hot`),
//! keyed by where their call is written in the source the app was built
//! from, and its views are built again with their state kept.
use proc_macro2::{Group, TokenStream, TokenTree};
use std::path::{Path, PathBuf};

#[allow(clippy::all, unused)]
#[path = "../../air-macro/src/lib.rs"]
pub(crate) mod air_rsx;

/// An edited template: its `rsx!` call's place in the built source (line,
/// column, 1-based), its index in the call, its markup now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edited {
    pub line: u32,
    pub col: u32,
    pub idx: u32,
    pub nodes: Vec<u8>,
    pub count: u32,
}

/// One `rsx!` call: where it is written, its shape, its templates.
struct Call {
    line: u32,
    col: u32,
    shape: String,
    made: Vec<air_rsx::Made>,
}

/// The templates of `now` that differ from `built` (the source the
/// running app was built from) when nothing else differs (pure): the code
/// outside the `rsx!` calls, and each call's holes, are the same. None: a
/// change the code must be built for (or a file that doesn't parse).
pub fn edited(built: &str, now: &str) -> Option<Vec<Edited>> {
    let (outside_b, calls_b) = read(built)?;
    let (outside_n, calls_n) = read(now)?;
    if outside_b != outside_n || calls_b.len() != calls_n.len() {
        return None;
    }
    let mut out = vec![];
    for (b, n) in calls_b.iter().zip(&calls_n) {
        if b.shape != n.shape || b.made.len() != n.made.len() {
            return None;
        }
        for (idx, (tb, tn)) in b.made.iter().zip(&n.made).enumerate() {
            if tb != tn {
                out.push(Edited {
                    line: b.line,
                    col: b.col,
                    idx: idx as u32,
                    nodes: tn.nodes.clone(),
                    count: tn.count,
                });
            }
        }
    }
    Some(out)
}

/// `src`'s code with each `rsx!` call's body left out, and its calls,
/// nested ones too, in the order they are written.
fn read(src: &str) -> Option<(String, Vec<Call>)> {
    let ts: TokenStream = src.parse().ok()?;
    let mut calls = vec![];
    let outside = walk(ts, &mut calls)?;
    Some((outside.to_string(), calls))
}

/// `ts` with every `rsx!` call's body emptied; the calls found go to `calls`.
fn walk(ts: TokenStream, calls: &mut Vec<Call>) -> Option<TokenStream> {
    let toks: Vec<TokenTree> = ts.into_iter().collect();
    let mut out = Vec::with_capacity(toks.len());
    for (i, t) in toks.iter().enumerate() {
        match t {
            TokenTree::Group(g) if i >= 2 && is_rsx(&toks[i - 2], &toks[i - 1]) => {
                let start = path_start(&toks, i - 2);
                let at = toks[start].span().start();
                // Its own shape: the calls inside it are calls of their own.
                let mut inner = vec![];
                let body = walk(g.stream(), &mut inner)?;
                let (shape, made) = air_rsx::templates(body).ok()?;
                calls.push(Call { line: at.line as u32, col: at.column as u32 + 1, shape, made });
                calls.append(&mut inner);
                out.push(TokenTree::Group(Group::new(g.delimiter(), TokenStream::new())));
            }
            TokenTree::Group(g) => {
                let mut ng = Group::new(g.delimiter(), walk(g.stream(), calls)?);
                ng.set_span(g.span());
                out.push(TokenTree::Group(ng));
            }
            t => out.push(t.clone()),
        }
    }
    Some(out.into_iter().collect())
}

fn is_rsx(name: &TokenTree, bang: &TokenTree) -> bool {
    matches!(name, TokenTree::Ident(n) if n == "rsx") && matches!(bang, TokenTree::Punct(p) if p.as_char() == '!')
}

/// Where the macro path ending at `name` starts (`air::rsx`, `::air::rsx`):
/// the compiler places the call there.
fn path_start(toks: &[TokenTree], name: usize) -> usize {
    let colon = |i: usize| matches!(toks.get(i), Some(TokenTree::Punct(p)) if p.as_char() == ':');
    let mut at = name;
    while at >= 2 && colon(at - 1) && colon(at - 2) {
        match at.checked_sub(3).map(|k| &toks[k]) {
            Some(TokenTree::Ident(_)) => at -= 3,
            _ => return at - 2,
        }
    }
    at
}

/// The file as the compiler names it in the app's build: from the
/// workspace's root (cargo builds a member there), else as it is.
pub fn compiler_name(file: &Path, workspace: &Path) -> String {
    file.strip_prefix(workspace).unwrap_or(file).to_string_lossy().replace('\\', "/")
}

/// The sources an app's build read, as it began (`snapshot`), to tell
/// what changed since in the running app's terms.
#[derive(Default)]
pub struct Built {
    files: std::collections::HashMap<PathBuf, (String, std::time::SystemTime)>,
}

impl Built {
    /// The `.rs` files under `dirs`, read now (a build starts).
    pub fn snapshot(dirs: &[PathBuf]) -> Self {
        Self::of(crate::hotpatch::sources_of(dirs), std::time::SystemTime::now())
    }

    /// `files` as a build that began `at` read them.
    pub fn of(files: std::collections::BTreeMap<PathBuf, String>, at: std::time::SystemTime) -> Self {
        Self { files: files.into_iter().map(|(f, t)| (f, (t, at))).collect() }
    }

    /// The text of `f` as the build read it.
    pub fn text(&self, f: &Path) -> Option<&str> {
        self.files.get(f).map(|(t, _)| t.as_str())
    }

    /// Whether `f` is a file the build read.
    pub fn has(&self, f: &Path) -> bool {
        self.files.contains_key(f)
    }

    /// The templates edited in `changed` since the build (each file's
    /// compiler name and its edits), if that is all that changed: every
    /// file a `.rs` read at the build, saved after it began.
    pub fn edits(&self, changed: &[PathBuf], workspace: &Path) -> Option<Vec<(String, Vec<Edited>)>> {
        changed
            .iter()
            .map(|f| {
                let (built, at) = self.files.get(f)?;
                let saved = std::fs::metadata(f).and_then(|m| m.modified()).ok()?;
                if saved < *at {
                    return None;
                }
                let now = std::fs::read_to_string(f).ok()?;
                Some((compiler_name(f, workspace), edited(built, &now)?))
            })
            .collect()
    }
}

/// The `Aio.hot` call's params for `edits` (base64 markup).
pub fn params(edits: &[(String, Vec<Edited>)]) -> aio::serde_json::Value {
    use base64::Engine;
    let b64 = &base64::engine::general_purpose::STANDARD;
    let files: Vec<&str> = edits.iter().map(|(f, _)| f.as_str()).collect();
    let templates: Vec<aio::serde_json::Value> = edits
        .iter()
        .flat_map(|(file, es)| {
            es.iter().map(move |e| {
                aio::serde_json::json!({
                    "file": file, "line": e.line, "col": e.col, "idx": e.idx,
                    "nodes": b64.encode(&e.nodes), "count": e.count,
                })
            })
        })
        .collect();
    aio::serde_json::json!({ "files": files, "templates": templates })
}

/// Send `edits` to the app's window over its `ui` door at `door`: how
/// many windows build their views again.
pub fn send(door: &str, edits: &[(String, Vec<Edited>)]) -> Result<u64, String> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let mut s = crate::cdp::Session::door(door).await?;
        let r = s.call(aio_shell::inspect::HOT, params(edits)).await?;
        r["windows"].as_u64().ok_or_else(|| format!("Aio.hot: {r}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = r#"use aio::ui::prelude::*;
pub fn app() -> View {
    let n = signal(|| 0);
    rsx! {
        h1 { class: "big", "Count" }
        p { "{n}" }
        button { onclick: move |_| n += 1, "Add" }
        if n.get() > 2 { span { "many" } } else { span { "few" } }
    }
}
fn other() -> View { aio::ui::prelude::rsx! { p { "x" } } }
"#;

    #[test]
    fn a_markup_edit_is_its_templates_and_a_code_edit_is_none() {
        assert_eq!(edited(APP, APP), Some(vec![]), "nothing changed");
        let text = edited(APP, &APP.replace("\"Add\"", "\"Add one\"")).unwrap();
        assert_eq!(text.len(), 1);
        assert_eq!((text[0].line, text[0].col, text[0].idx), (4, 5, 0), "the call's place, the outer template");
        let s = String::from_utf8_lossy(&text[0].nodes);
        assert!(s.contains("Add one") && s.contains("Count"), "the whole template now: {s}");
        // A branch's template: its own index, the call's place.
        let branch = edited(APP, &APP.replace("\"many\"", "\"lots\"")).unwrap();
        assert_eq!((branch[0].line, branch[0].idx), (4, 1));
        // Static attributes and elements are markup too.
        assert_eq!(edited(APP, &APP.replace("\"big\"", "\"huge\"")).map(|e| e.len()), Some(1));
        let more = APP.replace("\"Count\" }", "\"Count\", b { \"!\" } }");
        assert_eq!(edited(APP, &more).map(|e| e.len()), Some(1), "a static element added inside one");
        // The path form, placed at its start.
        let other = edited(APP, &APP.replace("p { \"x\" }", "p { \"y\" }")).unwrap();
        assert_eq!((other[0].line, other[0].col), (11, 22));
        // Code: a hole, a handler, anything outside the calls.
        for code in [
            APP.replace("\"{n}\"", "\"n = {n}\""),
            APP.replace("n += 1", "n += 2"),
            APP.replace("n.get() > 2", "n.get() > 3"),
            APP.replace("signal(|| 0)", "signal(|| 1)"),
            APP.replace("p { \"{n}\" }", "p { \"{n}\" } p { \"{n}\" }"),
            APP.replace("fn other", "fn another"),
            // A root more: the holes' nodes are counted from the roots.
            APP.replace("rsx! {", "rsx! {\n        hr {}\n"),
        ] {
            assert_eq!(edited(APP, &code), None, "{code}");
        }
        assert_eq!(edited(APP, "fn broken( {"), None, "a file that doesn't parse");
    }

    #[test]
    fn a_nested_call_is_its_own_and_lines_above_it_keep_the_built_place() {
        let src = "fn f() -> View {\n    rsx! { div { {Some(rsx! { b { \"in\" } })} } }\n}\n";
        let e = edited(src, &src.replace("\"in\"", "\"inner\"")).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!((e[0].line, e[0].col), (2, 24), "the nested call's own place: {e:?}");
        // Lines added above a call: its templates keyed where it was built.
        let moved = format!("// a note\n{}", APP.replace("\"Add\"", "\"Add one\""));
        assert_eq!(edited(APP, &moved).map(|e| e.iter().map(|e| e.line).collect::<Vec<_>>()), Some(vec![4]));
    }

    #[test]
    fn the_file_is_named_from_the_workspace_root() {
        let ws = Path::new("/w");
        assert_eq!(compiler_name(Path::new("/w/app/src/lib.rs"), ws), "app/src/lib.rs");
        assert_eq!(compiler_name(Path::new("/elsewhere/x.rs"), ws), "/elsewhere/x.rs");
    }
}

//! `ax tsgen <app>`: TypeScript types of an app's shared cells and their
//! `#[client]` methods, read from its source (as `ax lint` reads it), for
//! aio-rs's JS client (`crates/aio/client`): `client.cell(tally)` is then
//! typed, its state as serde writes it and each method's arguments and
//! answer, so a renamed method or a changed argument fails the page's type
//! check at the next `ax tsgen`, not at run time.
use std::collections::BTreeMap;
use std::fmt::Write;
use syn::{Attribute, Fields, FnArg, GenericArgument, Item, PathArguments, ReturnType, Type};

/// Where the types go unless `--out` says: beside the app's manifest.
pub const OUT: &str = "aio-types.ts";

/// What serde says of a container, field or variant.
#[derive(Default)]
struct Serde {
    rename: Option<String>,
    rename_all: Option<String>,
    tag: Option<String>,
    content: Option<String>,
    untagged: bool,
    transparent: bool,
    skip: bool,
    flatten: bool,
    optional: bool,
}

fn serde_of(attrs: &[Attribute]) -> Serde {
    let mut s = Serde::default();
    for a in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        _ = a.parse_nested_meta(|m| {
            let name = m.path.get_ident().map(|i| i.to_string()).unwrap_or_default();
            let text = || -> syn::Result<String> { Ok(m.value()?.parse::<syn::LitStr>()?.value()) };
            match name.as_str() {
                "rename" if m.input.peek(syn::Token![=]) => s.rename = Some(text()?),
                "rename_all" if m.input.peek(syn::Token![=]) => s.rename_all = Some(text()?),
                "tag" => s.tag = Some(text()?),
                "content" => s.content = Some(text()?),
                "untagged" => s.untagged = true,
                "transparent" => s.transparent = true,
                "skip" | "skip_serializing" => s.skip = true,
                "flatten" => s.flatten = true,
                "skip_serializing_if" => {
                    s.optional = true;
                    text()?;
                }
                _ => {
                    // Any other (`default`, `with = ..`, `rename(serialize = ..)`): read past it.
                    if m.input.peek(syn::Token![=]) {
                        m.value()?.parse::<syn::Expr>()?;
                    } else if m.input.peek(syn::token::Paren) {
                        m.parse_nested_meta(|n| {
                            if n.input.peek(syn::Token![=]) {
                                n.value()?.parse::<syn::Expr>()?;
                            }
                            Ok(())
                        })?;
                    }
                }
            }
            Ok(())
        });
    }
    s
}

/// `name` as serde's `rename_all = rule` writes it (pure): `variant` names
/// are PascalCase in Rust, field names snake_case.
pub fn renamed(name: &str, rule: &str, variant: bool) -> String {
    let snake = match variant {
        true => crate::lint::rust::snake(name),
        false => name.to_string(),
    };
    let pascal =
        || snake.split('_').map(|w| w[..w.len().min(1)].to_uppercase() + w.get(1..).unwrap_or("")).collect::<String>();
    match rule {
        "lowercase" => name.to_lowercase(),
        "UPPERCASE" => name.to_uppercase(),
        "PascalCase" if variant => name.to_string(),
        "PascalCase" => pascal(),
        "camelCase" => {
            let p = if variant { name.to_string() } else { pascal() };
            p[..p.len().min(1)].to_lowercase() + p.get(1..).unwrap_or("")
        }
        "snake_case" => snake,
        "SCREAMING_SNAKE_CASE" => snake.to_uppercase(),
        "kebab-case" => snake.replace('_', "-"),
        "SCREAMING-KEBAB-CASE" => snake.replace('_', "-").to_uppercase(),
        _ => name.to_string(),
    }
}

/// A TypeScript property name for `n`, quoted unless it is an identifier.
fn prop(n: &str) -> String {
    let ident = n.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if ident { n.to_string() } else { format!("{n:?}") }
}

/// The types it knows by name: the source's serialized ones.
type Known = BTreeMap<String, Item>;

fn args(seg: &syn::PathSegment) -> Vec<&Type> {
    match &seg.arguments {
        PathArguments::AngleBracketed(a) => a
            .args
            .iter()
            .filter_map(|g| match g {
                GenericArgument::Type(t) => Some(t),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

/// `t` as TypeScript, as serde_json writes it (pure).
fn ts(t: &Type, known: &Known) -> String {
    let wrap = |s: String| if s.contains(" | ") { format!("({s})") } else { s };
    match t {
        Type::Reference(r) => ts(&r.elem, known),
        Type::Paren(p) => ts(&p.elem, known),
        Type::Group(g) => ts(&g.elem, known),
        Type::Slice(s) => format!("{}[]", wrap(ts(&s.elem, known))),
        Type::Array(a) => format!("{}[]", wrap(ts(&a.elem, known))),
        Type::Tuple(t) if t.elems.is_empty() => "null".into(),
        Type::Tuple(t) => format!("[{}]", t.elems.iter().map(|e| ts(e, known)).collect::<Vec<_>>().join(", ")),
        Type::Path(p) => {
            let Some(seg) = p.path.segments.last() else { return "unknown".into() };
            let a = args(seg);
            let first = || a.first().map_or("unknown".into(), |t| ts(t, known));
            match seg.ident.to_string().as_str() {
                "bool" => "boolean".into(),
                "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128" | "usize"
                | "f32" | "f64" | "NonZeroU32" | "NonZeroU64" | "NonZeroUsize" => "number".into(),
                "String" | "str" | "char" | "Uuid" | "PathBuf" | "Path" | "DateTime" | "NaiveDate"
                | "NaiveDateTime" | "NaiveTime" | "SystemTime" | "IpAddr" | "SocketAddr" => "string".into(),
                "Option" => format!("{} | null", first()),
                "Vec" | "VecDeque" | "HashSet" | "BTreeSet" | "LinkedList" | "BinaryHeap" | "Vector" | "OrdSet" => {
                    format!("{}[]", wrap(first()))
                }
                "HashMap" | "BTreeMap" | "IndexMap" | "OrdMap" => {
                    format!("Record<string, {}>", a.get(1).map_or("unknown".into(), |t| ts(t, known)))
                }
                "Box" | "Arc" | "Rc" | "Cow" | "Cell" | "RefCell" => first(),
                // A field a viewer may not see (`visible`): its marker instead.
                "Hidden" => format!("{} | {{ $hidden: true }}", first()),
                "Duration" => "{ secs: number; nanos: number }".into(),
                n if known.contains_key(n) => n.into(),
                _ => "unknown".into(),
            }
        }
        _ => "unknown".into(),
    }
}

/// A struct's or variant's named fields as an object type (pure); `hide`
/// leaves out what is never sent (a cell's `#[server]` fields).
fn object(fields: &syn::FieldsNamed, rule: Option<&str>, known: &Known, hide: bool) -> String {
    let (mut props, mut flat) = (vec![], vec![]);
    for f in &fields.named {
        let s = serde_of(&f.attrs);
        if s.skip || (hide && f.attrs.iter().any(|a| a.path().is_ident("server"))) {
            continue;
        }
        let ty = ts(&f.ty, known);
        if s.flatten {
            flat.push(ty);
            continue;
        }
        let name = f.ident.as_ref().map(|i| i.to_string().trim_start_matches("r#").to_string()).unwrap_or_default();
        let name = s.rename.unwrap_or_else(|| rule.map_or(name.clone(), |r| renamed(&name, r, false)));
        props.push(format!("{}{}: {ty}", prop(&name), if s.optional { "?" } else { "" }));
    }
    let obj = if props.is_empty() { "{}".into() } else { format!("{{ {} }}", props.join("; ")) };
    std::iter::once(obj).chain(flat).collect::<Vec<_>>().join(" & ")
}

/// Unnamed fields: one is its value, more a tuple, none `null`.
fn tuple(fields: &syn::FieldsUnnamed, known: &Known) -> String {
    match fields.unnamed.len() {
        1 => ts(&fields.unnamed[0].ty, known),
        _ => format!("[{}]", fields.unnamed.iter().map(|f| ts(&f.ty, known)).collect::<Vec<_>>().join(", ")),
    }
}

/// One serialized type as a TypeScript declaration (pure).
fn decl(name: &str, item: &Item, known: &Known, cell: bool) -> String {
    match item {
        Item::Struct(s) => {
            let sd = serde_of(&s.attrs);
            let body = match &s.fields {
                Fields::Named(f) if sd.transparent => f.named.first().map_or("unknown".into(), |f| ts(&f.ty, known)),
                Fields::Named(f) => {
                    return format!("export type {name} = {};\n", object(f, sd.rename_all.as_deref(), known, cell));
                }
                Fields::Unnamed(f) => tuple(f, known),
                Fields::Unit => "null".into(),
            };
            format!("export type {name} = {body};\n")
        }
        Item::Enum(e) => {
            let sd = serde_of(&e.attrs);
            let arms: Vec<String> = e
                .variants
                .iter()
                .filter_map(|v| {
                    let vs = serde_of(&v.attrs);
                    if vs.skip {
                        return None;
                    }
                    let tag = vs.rename.clone().unwrap_or_else(|| {
                        sd.rename_all.as_deref().map_or(v.ident.to_string(), |r| renamed(&v.ident.to_string(), r, true))
                    });
                    let inner_rule = vs.rename_all.as_deref();
                    let body = match &v.fields {
                        Fields::Named(f) => Some(object(f, inner_rule, known, false)),
                        Fields::Unnamed(f) => Some(tuple(f, known)),
                        Fields::Unit => None,
                    };
                    let q = format!("{tag:?}");
                    Some(match (&sd.tag, &sd.content, sd.untagged, body) {
                        (_, _, true, b) => b.unwrap_or("null".into()),
                        (Some(t), Some(c), _, Some(b)) => format!("{{ {}: {q}; {}: {b} }}", prop(t), prop(c)),
                        (Some(t), _, _, None) => format!("{{ {}: {q} }}", prop(t)),
                        (Some(t), None, _, Some(b)) => match b.strip_prefix("{ ") {
                            Some(rest) => format!("{{ {}: {q}; {rest}", prop(t)),
                            None if b == "{}" => format!("{{ {}: {q} }}", prop(t)),
                            None => format!("{{ {}: {q} }} & {b}", prop(t)),
                        },
                        (None, _, _, None) => q,
                        (None, _, _, Some(b)) => format!("{{ {}: {b} }}", prop(&tag)),
                    })
                })
                .collect();
            let arms = if arms.is_empty() { "never".to_string() } else { arms.join(" | ") };
            format!("export type {name} = {arms};\n")
        }
        _ => String::new(),
    }
}

fn is(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|a| a.path().segments.last().is_some_and(|s| s.ident == name))
}

fn serialized(attrs: &[Attribute]) -> bool {
    is(attrs, "cell")
        || is(attrs, "data")
        || attrs.iter().filter(|a| a.path().is_ident("derive")).any(|a| {
            let mut yes = false;
            _ = a.parse_nested_meta(|m| {
                yes |= m.path.segments.last().is_some_and(|s| s.ident == "Serialize");
                Ok(())
            });
            yes
        })
}

/// A cell's flags: its id (`id = ".."`, else its name in snake case), its
/// version (1 unless set) and whether it is shared.
fn cell_flags(name: &str, attrs: &[Attribute]) -> Option<(String, u64, bool)> {
    let a = attrs.iter().find(|a| a.path().segments.last().is_some_and(|s| s.ident == "cell"))?;
    let (mut id, mut v, mut shared) = (crate::lint::rust::snake(name), 1, false);
    if let syn::Meta::List(_) = &a.meta {
        _ = a.parse_nested_meta(|m| {
            if m.path.is_ident("id") {
                id = m.value()?.parse::<syn::LitStr>()?.value();
            } else if m.path.is_ident("version") {
                v = m.value()?.parse::<syn::LitInt>()?.base10_parse()?;
            } else if m.path.is_ident("shared") {
                shared = true;
            } else if m.input.peek(syn::Token![=]) {
                m.value()?.parse::<syn::Expr>()?;
            } else if m.input.peek(syn::token::Paren) {
                m.parse_nested_meta(|n| {
                    if n.input.peek(syn::Token![=]) {
                        n.value()?.parse::<syn::Expr>()?;
                    }
                    Ok(())
                })?;
            }
            Ok(())
        });
    }
    Some((id, v, shared))
}

/// Every item of `items`, inline modules' too.
fn walk<'a>(items: &'a [Item], out: &mut Vec<&'a Item>) {
    for i in items {
        if let Item::Mod(m) = i
            && let Some((_, inner)) = &m.content
        {
            walk(inner, out);
        }
        out.push(i);
    }
}

/// A method's arguments and answer, as the wire carries them (pure):
/// `cx: &Cx` is the server's, not the caller's; `Result<T>` answers `T`.
fn method(f: &syn::ImplItemFn, known: &Known) -> String {
    let args: Vec<String> = f
        .sig
        .inputs
        .iter()
        .filter_map(|a| match a {
            FnArg::Typed(pt) => Some(pt),
            FnArg::Receiver(_) => None,
        })
        .filter(|pt| !matches!(&*pt.ty, Type::Reference(r) if matches!(&*r.elem, Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "Cx"))))
        .enumerate()
        .map(|(i, pt)| {
            let name = match &*pt.pat {
                syn::Pat::Ident(p) => p.ident.to_string().trim_start_matches('_').to_string(),
                _ => String::new(),
            };
            let name = if name.is_empty() { format!("arg{i}") } else { name };
            format!("{name}: {}", ts(&pt.ty, known))
        })
        .collect();
    let ok = match &f.sig.output {
        ReturnType::Default => "null".into(),
        ReturnType::Type(_, t) => match &**t {
            Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "Result") => {
                args_of_result(p.path.segments.last().unwrap(), known)
            }
            t => ts(t, known),
        },
    };
    format!("{}: {{ args: [{}]; ok: {ok} }}", prop(&f.sig.ident.to_string()), args.join(", "))
}

fn args_of_result(seg: &syn::PathSegment, known: &Known) -> String {
    args(seg).first().map_or("null".into(), |t| ts(t, known))
}

/// The id as a TypeScript const's name: `tally_log` is `tallyLog`.
fn const_name(id: &str) -> String {
    let c = renamed(&id.replace('-', "_"), "camelCase", false);
    if c.starts_with(|c: char| c.is_ascii_digit()) { format!("_{c}") } else { c }
}

/// The types file of app `app`, from its source `files` (path, text), pure.
pub fn render(app: &str, files: &[(String, String)]) -> Result<String, String> {
    let parsed: Vec<syn::File> =
        files.iter().map(|(p, t)| syn::parse_file(t).map_err(|e| format!("{p}: {e}"))).collect::<Result<_, _>>()?;
    let mut all = vec![];
    parsed.iter().for_each(|f| walk(&f.items, &mut all));
    let mut known = Known::new();
    for i in &all {
        match i {
            Item::Struct(s) if serialized(&s.attrs) => _ = known.insert(s.ident.to_string(), (*i).clone()),
            Item::Enum(e) if serialized(&e.attrs) => _ = known.insert(e.ident.to_string(), (*i).clone()),
            _ => {}
        }
    }
    let mut out = format!(
        "// Written by `ax tsgen {app}` from its source: its shared cells' state and\n\
         // their #[client] methods, for aio-rs's JS client. Run it again after a\n\
         // change; edits here are lost then.\n\
         import type {{ CellRef }} from \"aio-rs-client\";\n\n"
    );
    for (name, item) in &known {
        // A cell that isn't shared never reaches a client.
        let cell = match item {
            Item::Struct(s) => cell_flags(name, &s.attrs).map(|(.., shared)| shared),
            _ => None,
        };
        if cell != Some(false) {
            out += &decl(name, item, &known, cell.is_some());
        }
    }
    let mut cells = 0;
    for (name, item) in &known {
        let Item::Struct(s) = item else { continue };
        let Some((id, v, true)) = cell_flags(name, &s.attrs) else { continue };
        cells += 1;
        let methods: Vec<String> = all
            .iter()
            .filter_map(|i| match i {
                Item::Impl(im)
                    if is(&im.attrs, "methods") && matches!(&*im.self_ty, Type::Path(p) if p.path.is_ident(name)) =>
                {
                    Some(im)
                }
                _ => None,
            })
            .flat_map(|im| &im.items)
            .filter_map(|i| match i {
                syn::ImplItem::Fn(f) if is(&f.attrs, "client") => Some(method(f, &known)),
                _ => None,
            })
            .collect();
        let mut m = String::new();
        methods.iter().for_each(|x| _ = writeln!(m, "    {x};"));
        _ = write!(
            out,
            "\nexport const {}: CellRef<{{\n  state: {name};\n  methods: {{\n{m}  }};\n}}> = {{ id: {id:?}, v: {v} }};\n",
            const_name(&id)
        );
    }
    match cells {
        0 => Err(format!("{app} has no #[cell(shared)] cell: nothing reaches a client")),
        _ => Ok(out),
    }
}

/// The app's Rust files under `dir/src`, as (path, text).
pub fn sources(dir: &std::path::Path) -> Result<Vec<(String, String)>, String> {
    fn walk(d: &std::path::Path, out: &mut Vec<(String, String)>) -> Result<(), String> {
        let mut es: Vec<_> = std::fs::read_dir(d).map_err(|e| format!("{}: {e}", d.display()))?.flatten().collect();
        es.sort_by_key(|e| e.path());
        for e in es {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out)?;
            } else if p.extension().is_some_and(|x| x == "rs") {
                let t = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                out.push((p.display().to_string(), t));
            }
        }
        Ok(())
    }
    let mut out = vec![];
    walk(&dir.join("src"), &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_names_are_renamed_as_serde_does() {
        for (rule, field, variant) in [
            ("camelCase", "dueAt", "dueAt"),
            ("PascalCase", "DueAt", "DueAt"),
            ("snake_case", "due_at", "due_at"),
            ("SCREAMING_SNAKE_CASE", "DUE_AT", "DUE_AT"),
            ("kebab-case", "due-at", "due-at"),
            ("SCREAMING-KEBAB-CASE", "DUE-AT", "DUE-AT"),
            ("lowercase", "due_at", "dueat"),
            ("UPPERCASE", "DUE_AT", "DUEAT"),
        ] {
            assert_eq!(renamed("due_at", rule, false), field, "{rule}");
            assert_eq!(renamed("DueAt", rule, true), variant, "{rule}");
        }
    }

    #[test]
    fn the_fixture_s_types_are_the_committed_ones() {
        let src = include_str!("../../aio/client/test/types/fixture.rs");
        let got = render("fixture", &[("fixture.rs".into(), src.into())]).unwrap();
        let want = include_str!("../../aio/client/test/types/fixture.ts");
        assert_eq!(got, want, "run: ax tsgen's render on fixture.rs, and check test/types/use.ts against it");
    }

    #[test]
    fn an_app_with_no_shared_cell_is_said() {
        let e = render("x", &[("a.rs".into(), "#[cell] pub struct A { n: u32 }".into())]).unwrap_err();
        assert!(e.contains("no #[cell(shared)] cell"), "{e}");
        assert!(render("x", &[("a.rs".into(), "fn (".into())]).unwrap_err().starts_with("a.rs: "));
    }
}

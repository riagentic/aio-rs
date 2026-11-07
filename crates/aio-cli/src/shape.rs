//! What a hot-patch can't change in a running app: a type's definition (a
//! cell's fields, an `#[aio::data]` type, an enum's variants), a static or
//! a thread-local, a function's signature, an item-level macro call
//! (`aio::client!`, `thread_local!`). A patch swaps function bodies only:
//! the running process keeps its values in the old layout, so a field
//! added to a cell and patched in reads memory of another shape (air's
//! "a global of another type", or worse). `ax dev` compares a saved file's
//! shape with the running build's and restarts on a difference, saying
//! which item changed; a body edit, a comment or a `use` is still patched.
use quote::ToTokens;
use std::collections::BTreeMap;

/// The items of a file whose shape differs between `before` (the running
/// build's) and `after` (as saved), as a person names them (`struct
/// Counter`, `fn Counter::add`), sorted. `None`: one of them doesn't parse
/// (the compiler will say why).
pub fn changed(before: &str, after: &str) -> Option<Vec<String>> {
    let (a, b) = (shape(before)?, shape(after)?);
    let mut out: Vec<String> = a.iter().filter(|(k, v)| b.get(*k) != Some(v)).map(|(k, _)| k.clone()).collect();
    out.extend(b.keys().filter(|k| !a.contains_key(*k) && is_kept(k)).cloned());
    // A method is a function as a person names it.
    let mut out: Vec<String> = out.into_iter().map(|k| k.replacen(METHOD, "fn ", 1)).collect();
    out.sort();
    out.dedup();
    Some(out)
}

/// A function of an `impl` block, apart from one of an inline module.
const METHOD: &str = "method ";

/// A new item that changes the running app's shape: a new function (in a
/// module or a function body too), type, or const is the patch's own; a
/// new static or thread-local is too, but a new item-level macro call (a new
/// cell's client, a `thread_local!`), a new module file, or a new method of
/// a type the app runs (its dispatch is generated) changes what the build
/// generated for code already running.
fn is_kept(name: &str) -> bool {
    [METHOD, "macro ", "mod "].iter().any(|p| name.starts_with(p))
}

/// Each item's shape by name (pure).
fn shape(text: &str) -> Option<BTreeMap<String, String>> {
    let file = syn::parse_file(text).ok()?;
    let mut out = BTreeMap::new();
    items(&file.items, "", &mut out);
    Some(out)
}

fn items(list: &[syn::Item], at: &str, out: &mut BTreeMap<String, String>) {
    // Each macro path's calls counted apart: an item added elsewhere renames none.
    let mut calls: BTreeMap<String, usize> = BTreeMap::new();
    for item in list {
        let mut item = item.clone();
        strip_docs(&mut item);
        let tokens = |t: &dyn ToTokens| t.to_token_stream().to_string();
        match &mut item {
            syn::Item::Fn(f) => {
                items(&body_items(&f.block), &format!("{at}{}::", f.sig.ident), out);
                *f.block = syn::parse_quote!({});
                out.insert(format!("fn {at}{}", f.sig.ident), tokens(f));
            }
            syn::Item::Impl(imp) => {
                let ty = imp.self_ty.to_token_stream().to_string().replace(' ', "");
                let tr = imp.trait_.as_ref().map(|(_, p, _)| p.to_token_stream().to_string().replace(' ', ""));
                let name = tr.map_or_else(|| ty.clone(), |t| format!("<{ty} as {t}>"));
                for it in &mut imp.items {
                    if let syn::ImplItem::Fn(f) = it {
                        items(&body_items(&f.block), &format!("{at}{name}::{}::", f.sig.ident), out);
                        f.block = syn::parse_quote!({});
                        out.insert(format!("{METHOD}{at}{name}::{}", f.sig.ident), tokens(f));
                    }
                }
                let mut head = imp.clone();
                head.items.retain(|it| !matches!(it, syn::ImplItem::Fn(_)));
                out.insert(format!("impl {at}{name}"), tokens(&head));
            }
            syn::Item::Mod(m) => match m.content.take() {
                Some((_, inner)) => items(&inner, &format!("{at}{}::", m.ident), out),
                None => _ = out.insert(format!("mod {at}{}", m.ident), tokens(m)),
            },
            syn::Item::Struct(x) => _ = out.insert(format!("struct {at}{}", x.ident), tokens(x)),
            syn::Item::Enum(x) => _ = out.insert(format!("enum {at}{}", x.ident), tokens(x)),
            syn::Item::Union(x) => _ = out.insert(format!("union {at}{}", x.ident), tokens(x)),
            syn::Item::Type(x) => _ = out.insert(format!("type {at}{}", x.ident), tokens(x)),
            syn::Item::Static(x) => _ = out.insert(format!("static {at}{}", x.ident), tokens(x)),
            syn::Item::Const(x) => _ = out.insert(format!("const {at}{}", x.ident), tokens(x)),
            syn::Item::Trait(x) => _ = out.insert(format!("trait {at}{}", x.ident), tokens(x)),
            syn::Item::Macro(x) => {
                let path = x.mac.path.to_token_stream().to_string().replace(' ', "");
                // `macro_rules!` by its name; a call by its path and its turn among those.
                let n = calls.entry(path.clone()).or_default();
                *n += 1;
                let name = x.ident.as_ref().map_or_else(|| format!("{path}#{n}"), |id| format!("{path} {id}"));
                out.insert(format!("macro {at}{name}"), tokens(x));
            }
            // `use`, `extern crate`, a foreign block: no layout of their own.
            _ => {}
        }
    }
}

/// The items declared in a function's body (a type, a static, an `impl`),
/// at any depth of its blocks; each one's own body is [`items`]'.
fn body_items(block: &syn::Block) -> Vec<syn::Item> {
    struct Found(Vec<syn::Item>);
    impl<'a> syn::visit::Visit<'a> for Found {
        fn visit_item(&mut self, i: &'a syn::Item) {
            self.0.push(i.clone());
        }
    }
    let mut found = Found(vec![]);
    syn::visit::Visit::visit_block(&mut found, block);
    found.0
}

/// Doc comments are no shape: editing one patches.
fn strip_docs(item: &mut syn::Item) {
    fn keep(attrs: &mut Vec<syn::Attribute>) {
        attrs.retain(|a| !a.path().is_ident("doc"));
    }
    match item {
        syn::Item::Fn(x) => keep(&mut x.attrs),
        syn::Item::Struct(x) => {
            keep(&mut x.attrs);
            x.fields.iter_mut().for_each(|f| keep(&mut f.attrs));
        }
        syn::Item::Enum(x) => {
            keep(&mut x.attrs);
            for v in &mut x.variants {
                keep(&mut v.attrs);
                v.fields.iter_mut().for_each(|f| keep(&mut f.attrs));
            }
        }
        syn::Item::Impl(x) => {
            keep(&mut x.attrs);
            for it in &mut x.items {
                if let syn::ImplItem::Fn(f) = it {
                    keep(&mut f.attrs);
                }
            }
        }
        syn::Item::Union(x) => keep(&mut x.attrs),
        syn::Item::Type(x) => keep(&mut x.attrs),
        syn::Item::Static(x) => keep(&mut x.attrs),
        syn::Item::Const(x) => keep(&mut x.attrs),
        syn::Item::Trait(x) => keep(&mut x.attrs),
        syn::Item::Mod(x) => keep(&mut x.attrs),
        syn::Item::Macro(x) => keep(&mut x.attrs),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: &str = r#"
use aio::ui::prelude::*;

/// What the app keeps.
#[aio::cell(persist, coalesce, id = "counter")]
pub struct Counter {
    pub count: i64,
}

#[aio::methods]
impl Counter {
    /// Count `by` more.
    pub fn add(&mut self, by: i64) {
        self.count += by;
    }
}

aio::client!(Counter, counter);

#[component]
pub fn App() -> View {
    rsx! { h1 { "hi" } }
}
"#;

    fn edit(from: &str, to: &str) -> Vec<String> {
        assert!(CELL.contains(from), "{from}");
        changed(CELL, &CELL.replacen(from, to, 1)).unwrap()
    }

    #[test]
    fn a_body_a_comment_a_doc_or_a_use_is_patched() {
        assert!(edit("self.count += by;", "self.count += by * 2;").is_empty());
        assert!(edit(r#"h1 { "hi" }"#, r#"h1 { "hello" } p { "more" }"#).is_empty());
        assert!(edit("/// Count `by` more.", "/// Count `by` more, or less.").is_empty());
        assert!(edit("/// What the app keeps.", "/// What the app keeps // and more").is_empty());
        assert!(edit("use aio::ui::prelude::*;", "use aio::ui::prelude::*;\nuse std::fmt::Write;").is_empty());
        // A new free function is the patch's own.
        assert!(edit("aio::client!", "fn helper() -> i64 { 2 }\naio::client!").is_empty());
    }

    #[test]
    fn a_cells_field_a_signature_a_new_method_or_a_client_restarts() {
        assert_eq!(edit("    pub count: i64,", "    pub count: i64,\n    pub label: String,"), ["struct Counter"]);
        assert_eq!(edit("    pub count: i64,", "    pub count: i32,"), ["struct Counter"]);
        assert_eq!(edit("persist, coalesce", "persist"), ["struct Counter"]);
        assert_eq!(edit("by: i64)", "by: i32)"), ["fn Counter::add"]);
        assert_eq!(
            edit(
                "    pub fn add(",
                "    pub fn double(&mut self) {\n        self.count *= 2;\n    }\n\n    pub fn add("
            ),
            ["fn Counter::double"]
        );
        assert_eq!(edit("aio::client!(Counter, counter);", "aio::client!(Counter, count);"), ["macro aio::client#1"]);
        assert_eq!(edit("pub fn App()", "pub fn App(n: i64)"), ["fn App"]);
        assert_eq!(
            edit("aio::client!", "static N: i64 = 1;\naio::client!").len(),
            0,
            "a new static is the patch's own"
        );
        let enumed = "enum E { A }\n";
        assert_eq!(changed(enumed, "enum E { A, B }\n").unwrap(), ["enum E"]);
        let inline = "mod m { pub struct S { a: u8 } }\n";
        assert_eq!(changed(inline, "mod m { pub struct S { a: u16 } }\n").unwrap(), ["struct m::S"]);
    }

    #[test]
    fn a_new_function_in_a_module_is_the_patchs_own_a_new_method_is_not() {
        let tests = "fn a() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn one() {}\n}\n";
        let more = tests.replace("fn one() {}", "fn one() {}\n    #[test]\n    fn two() {}");
        assert!(changed(tests, &more).unwrap().is_empty(), "a new test");
        let inline = "mod m { pub fn f() {} }\n";
        assert!(changed(inline, "mod m { pub fn f() {} pub fn g() {} }\n").unwrap().is_empty());
        let imp = "struct S;\nimpl S { fn f(&self) {} }\n";
        assert_eq!(changed(imp, "struct S;\nimpl S { fn f(&self) {} fn g(&self) {} }\n").unwrap(), ["fn S::g"]);
    }

    #[test]
    fn an_item_in_a_functions_body_has_its_shape_too() {
        let body =
            "fn view() -> u8 {\n    struct Row { a: u8 }\n    if true { static N: u8 = 1; }\n    Row { a: 1 }.a\n}\n";
        assert_eq!(changed(body, &body.replace("a: u8", "a: u16")).unwrap(), ["struct view::Row"]);
        assert_eq!(changed(body, &body.replace("N: u8", "N: u16")).unwrap(), ["static view::N"], "in a block");
        assert!(changed(body, &body.replace("Row { a: 1 }.a", "Row { a: 2 }.a")).unwrap().is_empty());
        let method = "struct S;\nimpl S {\n    fn f(&self) { enum E { A } }\n}\n";
        assert_eq!(changed(method, &method.replace("{ A }", "{ A, B }")).unwrap(), ["enum S::f::E"]);
    }

    #[test]
    fn code_that_doesnt_parse_has_no_shape_to_compare() {
        assert_eq!(changed(CELL, "pub struct {"), None);
    }
}

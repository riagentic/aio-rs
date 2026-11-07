//! air's `rsx!` and `#[component]`: elements as `tag { attr: v, children }`,
//! its output (its paths start at `air`: the crate, or the name `aio::ui::prelude`
//! brings in, so an app needs no `air` dependency of its own):
//! one static template per element tree (its markup pre-encoded as a byte
//! string: one token, whatever its size) and one call per dynamic hole.
//! No syn: the input is split on token trees, expressions are passed
//! through as written (their spans kept), so the macro runs in
//! microseconds and its expansion stays near the size of its input.
//!
//! `ax dev` compiles this file too (not as a proc-macro: `cfg(proc_macro)`
//! is off there) to read an edited `rsx!` as the compiler will:
//! `templates` says whether only its markup changed (its holes the same),
//! and what its templates are now (template hot reload).
// Read by `ax dev` (and by this crate's tests) without the macros: not all of it is used there.
#![cfg_attr(not(proc_macro), allow(dead_code))]
use proc_macro2::{Delimiter, Ident, Literal, Spacing, Span, TokenStream, TokenTree};
use quote::{ToTokens, format_ident, quote, quote_spanned};
use std::cell::RefCell;

/// Elements, text, `{expr}`, components, `for … in … { … }` (keyed rows:
/// `key:` on the row), `if … { … } else { … }`. A
/// literal is static; a `"{x}"` string and every other expression is a
/// hole that follows what it reads (the names it uses are cloned in).
#[cfg(proc_macro)]
#[proc_macro]
pub fn rsx(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    // Where it is written: its templates' key for a hot reload (`ax dev`).
    let at = proc_macro::Span::call_site();
    let at = (at.file().replace('\\', "/"), at.line() as u32, at.column() as u32);
    expand(TokenStream::from(input), Some(at)).into()
}

/// `rsx!`'s expansion of `input`, written at `at` (file, line, column of
/// the call, 1-based).
pub(crate) fn expand(input: TokenStream, at: Option<(String, u32, u32)>) -> TokenStream {
    let toks: Vec<TokenTree> = input.into_iter().collect();
    HOT.with_borrow_mut(|h| *h = Hot { at, ..Hot::default() });
    match parse_nodes(&toks) {
        Ok(nodes) => body(&nodes),
        Err((span, msg)) => quote_spanned!(span=> ::core::compile_error!(#msg)),
    }
}

/// One template of an `rsx!` as the page gets it (air's `Template`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Made {
    pub(crate) nodes: Vec<u8>,
    pub(crate) count: u32,
}

#[derive(Default)]
struct Hot {
    at: Option<(String, u32, u32)>,
    /// The templates made so far, by index (`templates`).
    made: Option<Vec<Made>>,
    next: u32,
}

thread_local!(static HOT: RefCell<Hot> = RefCell::default());

// Inside an `<svg>` of the call being expanded: a template made there (a
// row, a branch) puts its root elements in SVG's namespace.
thread_local!(static IN_SVG: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });

/// Elements only SVG has: a template rooted at one (a component's `circle`,
/// a row's `g`) is SVG's wherever it goes. Shared names (`a`, `title`,
/// `style`, `script`) are SVG's only inside an `svg` of the same `rsx!`.
const SVG_ONLY: &[&str] = &[
    "animate",
    "animateMotion",
    "animateTransform",
    "circle",
    "clipPath",
    "defs",
    "desc",
    "ellipse",
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feConvolveMatrix",
    "feDiffuseLighting",
    "feDisplacementMap",
    "feDistantLight",
    "feDropShadow",
    "feFlood",
    "feFuncA",
    "feFuncB",
    "feFuncG",
    "feFuncR",
    "feGaussianBlur",
    "feImage",
    "feMerge",
    "feMergeNode",
    "feMorphology",
    "feOffset",
    "fePointLight",
    "feSpecularLighting",
    "feSpotLight",
    "feTile",
    "feTurbulence",
    "filter",
    "foreignObject",
    "g",
    "image",
    "line",
    "linearGradient",
    "marker",
    "mask",
    "metadata",
    "mpath",
    "path",
    "pattern",
    "polygon",
    "polyline",
    "radialGradient",
    "rect",
    "set",
    "stop",
    "switch",
    "symbol",
    "text",
    "textPath",
    "tspan",
    "use",
    "view",
];

/// The `rsx!` call `input` read for a hot reload: its shape (the
/// expansion with its templates' markup left out: two calls of one shape
/// differ in markup alone, so the running code fits the new markup) and
/// its templates, in the order their indices are given. Err: it doesn't parse.
#[cfg(not(proc_macro))]
pub(crate) fn templates(input: TokenStream) -> Result<(String, Vec<Made>), String> {
    let toks: Vec<TokenTree> = input.into_iter().collect();
    HOT.with_borrow_mut(|h| *h = Hot { made: Some(vec![]), ..Hot::default() });
    let shape = parse_nodes(&toks).map(|nodes| body(&nodes).to_string()).map_err(|(_, msg)| msg);
    let made = HOT.with_borrow_mut(|h| h.made.take()).unwrap_or_default();
    Ok((shape?, made))
}

/// The `rsx!` call `input` as the compiler reads it, for `ax lint`. Err:
/// it doesn't parse (the compiler says why).
#[cfg(not(proc_macro))]
pub(crate) fn nodes(input: TokenStream) -> Result<Vec<Node>, String> {
    parse_nodes(&input.into_iter().collect::<Vec<_>>()).map_err(|(_, msg)| msg)
}

/// `fn Row(name: String, on: Signal<bool>) -> View`: a props struct
/// `RowProps { pub name, pub on }` with its builder, and
/// `fn Row(RowProps { name, on }: RowProps)`, called as `Row { name: …, on }`
/// in `rsx!`. No arguments: a plain `fn() -> View`. Props:
/// `#[props(default)]` / `#[props(default = e)]` may be left out, an
/// `Option<T>` is `None` unless given (and takes a `T`), a `View` is empty,
/// `#[props(extends = GlobalAttributes)] attributes: Vec<Attribute>` takes
/// the attributes given (`"data-t": …`, `id`, `..attributes`); a `Prop`
/// follows what it is given. Any other prop is required (a compile error
/// without it).
#[cfg(proc_macro)]
#[proc_macro_attribute]
pub fn component(_: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let toks: Vec<TokenTree> = TokenStream::from(item).into_iter().collect();
    let Some(fn_at) = toks.iter().position(|t| is_ident(t, "fn")) else {
        return quote!(::core::compile_error!("air: #[component] goes on a fn")).into();
    };
    let Some(TokenTree::Ident(name)) = toks.get(fn_at + 1).cloned() else {
        return quote!(::core::compile_error!("air: #[component] goes on a `fn Name(props…) -> View`")).into();
    };
    // `<R: Clone + 'static>`: the props are generic as the fn is.
    let (generics, mut at) = (generic_params(&toks, fn_at + 2), fn_at + 2);
    if let Some((_, end)) = &generics {
        at = *end;
    }
    let args = match toks.get(at) {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => g.stream(),
        _ => return quote!(::core::compile_error!("air: #[component] goes on a `fn Name(props…) -> View`")).into(),
    };
    let (gp, gn) = match &generics {
        Some((params, _)) => split_generics(params),
        None => (quote!(), quote!()),
    };
    let head: TokenStream = toks[..fn_at].iter().cloned().collect();
    let rest: TokenStream = toks[at + 1..].iter().cloned().collect();
    let args: Vec<Vec<TokenTree>> = split_top(&args.into_iter().collect::<Vec<_>>());
    if args.is_empty() {
        return quote!(#[allow(non_snake_case)] #head fn #name<#gp>() #rest).into();
    }
    let props = format_ident!("{}Props", name);
    let fields: Vec<Field> = match args.iter().map(|a| field(a)).collect::<Res<_>>() {
        Ok(f) => f,
        Err((span, msg)) => return quote_spanned!(span=> ::core::compile_error!(#msg);).into(),
    };
    let vis = toks[..fn_at].iter().find(|t| is_ident(t, "pub")).map(|_| quote!(pub));
    let decl = fields.iter().map(|f| {
        let (docs, n, ty) = (&f.docs, &f.name, &f.ty);
        quote!(#(#docs)* pub #n: #ty)
    });
    let pats = fields.iter().map(|f| &f.pat);
    let builder = builder(vis.clone(), &props, &name.to_string(), &fields, &gp, &gn);
    quote!(
        #[derive(Clone)]
        #vis struct #props<#gp> { #(#decl),* }
        #builder
        #[allow(non_snake_case)]
        #head fn #name<#gp>(#props { #(#pats),* }: #props<#gn>) #rest
    )
    .into()
}

/// The builder of a hand-written props struct (as `#[component]` makes for
/// its own): `#[props(…)]` on its fields as there.
#[cfg(proc_macro)]
#[proc_macro_derive(Props, attributes(props))]
pub fn props(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let toks: Vec<TokenTree> = TokenStream::from(item).into_iter().collect();
    let at = toks.iter().position(|t| is_ident(t, "struct"));
    let (name, body) = match at.map(|a| (toks.get(a + 1), toks.get(a + 2))) {
        Some((Some(TokenTree::Ident(n)), Some(TokenTree::Group(g)))) if g.delimiter() == Delimiter::Brace => {
            (n.clone(), g.stream())
        }
        _ => return quote!(::core::compile_error!("air: #[derive(Props)] goes on a struct with named fields (no generics)");).into(),
    };
    let vis = toks[..at.unwrap_or(0)].iter().find(|t| is_ident(t, "pub")).map(|_| quote!(pub));
    let fields: Vec<Field> =
        match split_top(&body.into_iter().collect::<Vec<_>>()).iter().map(|a| field(a)).collect::<Res<_>>() {
            Ok(f) => f,
            Err((span, msg)) => return quote_spanned!(span=> ::core::compile_error!(#msg);).into(),
        };
    builder(vis, &name, &name.to_string(), &fields, &quote!(), &quote!()).into()
}

/// `<…>` at `at` (a fn's generics): its tokens, and where it ends.
fn generic_params(toks: &[TokenTree], at: usize) -> Option<(Vec<TokenTree>, usize)> {
    if !is_punct(toks.get(at), '<') {
        return None;
    }
    let mut depth = 0i32;
    for (k, t) in toks.iter().enumerate().skip(at) {
        match t {
            TokenTree::Punct(p) if p.as_char() == '<' => depth += 1,
            TokenTree::Punct(p) if p.as_char() == '>' && !is_punct(toks.get(k - 1), '-') => {
                depth -= 1;
                if depth == 0 {
                    return Some((toks[at + 1..k].to_vec(), k + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// Generic parameters with their bounds, and their names (each with a trailing comma).
fn split_generics(params: &[TokenTree]) -> (TokenStream, TokenStream) {
    let (mut gp, mut gn) = (quote!(), quote!());
    for p in split_top(params) {
        let decl: TokenStream = p.iter().cloned().collect();
        let name: TokenStream = match p.as_slice() {
            [TokenTree::Punct(q), TokenTree::Ident(l), ..] if q.as_char() == '\'' => quote!(#q #l),
            [t, ..] if is_ident(t, "const") => p.get(1).cloned().into_iter().collect(),
            [t, ..] => t.clone().into(),
            [] => continue,
        };
        gp.extend(quote!(#decl,));
        gn.extend(quote!(#name,));
    }
    (gp, gn)
}

/// A prop: its docs, its `#[props(…)]`, its pattern (in a component's
/// arguments), its name and type.
struct Field {
    docs: Vec<TokenStream>,
    pat: TokenStream,
    name: Ident,
    ty: TokenStream,
    kind: Kind,
    default: Option<TokenStream>,
    extends: bool,
}

/// What a prop's type makes of it (by its type's name).
enum Kind {
    Plain,
    View,
    /// `Option<T>`: `T`.
    Opt(TokenStream),
    /// `Option<View>`.
    OptView,
    /// `Prop<T>`: `T`; `Prop<Option<T>>`: `T`, true.
    Prop(TokenStream, bool),
}

/// `Name<inner>` of a type: `Name` (the last segment) and `inner`.
fn ty_head(ty: &[TokenTree]) -> (Option<String>, Option<TokenStream>) {
    let lt = ty.iter().position(|t| is_punct(Some(t), '<'));
    let head = ty[..lt.unwrap_or(ty.len())].iter().rev().find_map(|t| match t {
        TokenTree::Ident(i) => Some(i.to_string()),
        _ => None,
    });
    let inner = lt.and_then(|l| {
        let end = ty.iter().rposition(|t| is_punct(Some(t), '>'))?;
        (end > l).then(|| ty[l + 1..end].iter().cloned().collect())
    });
    (head, inner)
}

fn is_callback(head: Option<&str>) -> bool {
    matches!(head, Some("EventHandler" | "Callback"))
}

fn field(toks: &[TokenTree]) -> Res<Field> {
    let (mut docs, mut default, mut extends, mut i) = (vec![], None, false, 0);
    while is_punct(toks.get(i), '#') {
        let Some(TokenTree::Group(g)) = toks.get(i + 1) else { break };
        let inner: Vec<TokenTree> = g.stream().into_iter().collect();
        if inner.first().is_some_and(|t| is_ident(t, "props")) {
            if let Some(TokenTree::Group(args)) = inner.get(1) {
                for a in split_top(&args.stream().into_iter().collect::<Vec<_>>()) {
                    match a.as_slice() {
                        [d] if is_ident(d, "default") => default = Some(quote!(::core::default::Default::default())),
                        [d, eq, e @ ..] if is_ident(d, "default") && is_punct(Some(eq), '=') => {
                            default = Some(e.iter().cloned().collect())
                        }
                        [x, eq, ..] if is_ident(x, "extends") && is_punct(Some(eq), '=') => extends = true,
                        _ => {}
                    }
                }
            }
        } else if inner.first().is_some_and(|t| is_ident(t, "doc")) {
            let (h, g) = (&toks[i], &toks[i + 1]);
            docs.push(quote!(#h #g));
        }
        i += 2;
    }
    // A struct field's visibility.
    if toks.get(i).is_some_and(|t| is_ident(t, "pub")) {
        i += 1;
        if matches!(toks.get(i), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis) {
            i += 1;
        }
    }
    let rest = &toks[i..];
    let colon = rest.iter().enumerate().position(|(k, _)| is_colon(rest, k));
    let Some(colon) = colon else { return Err((span_of(toks, i), "air: a prop is `name: Type`".into())) };
    let pat: TokenStream = rest[..colon].iter().cloned().collect();
    let name = rest[..colon]
        .iter()
        .rev()
        .find_map(|t| match t {
            TokenTree::Ident(i) => Some(i.clone()),
            _ => None,
        })
        .ok_or((span_of(rest, 0), "air: a prop is `name: Type`".to_string()))?;
    let ty_toks = &rest[colon + 1..];
    let ty: TokenStream = ty_toks.iter().cloned().collect();
    let kind = match ty_head(ty_toks) {
        (Some(h), Some(inner)) if h == "Option" => match ty_head(&inner.clone().into_iter().collect::<Vec<_>>()) {
            (Some(i), None) if i == "View" => Kind::OptView,
            _ => Kind::Opt(inner),
        },
        (Some(h), Some(inner)) if h == "Prop" => {
            let it: Vec<TokenTree> = inner.clone().into_iter().collect();
            match ty_head(&it) {
                (Some(h), Some(inner2)) if h == "Option" => Kind::Prop(inner2, true),
                _ => Kind::Prop(inner, false),
            }
        }
        (Some(h), None) if h == "View" => Kind::View,
        _ => Kind::Plain,
    };
    Ok(Field { docs, pat, name, ty, kind, default, extends })
}

/// `props`'s builder: `Option`s of its fields, one type parameter per
/// required prop (`()` until given, then `Set`), `build` once all are.
fn builder(
    vis: Option<TokenStream>,
    props: &Ident,
    shown: &str,
    fields: &[Field],
    gp: &TokenStream,
    gn: &TokenStream,
) -> TokenStream {
    let b = format_ident!("{}Builder", props);
    let required: Vec<usize> = (0..fields.len())
        .filter(|&k| {
            fields[k].default.is_none()
                && !fields[k].extends
                && matches!(fields[k].kind, Kind::Plain | Kind::Prop(_, false))
        })
        .collect();
    let params: Vec<Ident> = (0..required.len()).map(|k| format_ident!("__R{}", k)).collect();
    let names: Vec<&Ident> = fields.iter().map(|f| &f.name).collect();
    let tys: Vec<&TokenStream> = fields.iter().map(|f| &f.ty).collect();
    let unit = params.iter().map(|_| quote!(()));
    let air = quote!(air);
    let mut generic_names = vec![];
    idents_in(&gn.clone().into_iter().collect::<Vec<_>>(), &mut generic_names);

    // Setters: `n(f)` takes a closure of the value (a `Prop` keeps it),
    // `__fn_n(v)` the value itself (a closure given to a handler prop).
    let setter = |k: usize| -> (TokenStream, TokenStream) {
        let f = &fields[k];
        let (n, ty) = (&f.name, &f.ty);
        let fname = format_ident!("__fn_{}", n.to_string().trim_start_matches("r#"));
        let (head, _) = ty_head(&ty.clone().into_iter().collect::<Vec<_>>());
        let (opt_cb, inner_cb) = match &f.kind {
            Kind::Opt(i) => (true, is_callback(ty_head(&i.clone().into_iter().collect::<Vec<_>>()).0.as_deref())),
            _ => (false, false),
        };
        let cb = is_callback(head.as_deref()) || inner_cb;
        // A type of the component's own parameters (`Vec<R>`): taken as it is,
        // so the value names the parameter.
        let mut used = vec![];
        idents_in(&ty.clone().into_iter().collect::<Vec<_>>(), &mut used);
        let exact = used.iter().any(|u| generic_names.contains(u));
        let (gens, param, val, fgen, fparam, fval) = match &f.kind {
            Kind::Prop(i, opt) if exact => {
                let t = if *opt { quote!(::core::option::Option<#i>) } else { quote!(#i) };
                (
                    quote!(),
                    quote!(f: impl ::core::ops::Fn() -> #t + 'static),
                    quote!(#air::Prop::derive(::std::boxed::Box::new(f))),
                    quote!(),
                    quote!(v: #t),
                    quote!(#air::Prop::Const(v)),
                )
            }
            _ if exact => (
                quote!(),
                quote!(f: impl ::core::ops::FnOnce() -> #ty),
                quote!(#air::untrack(f)),
                quote!(),
                quote!(v: #ty),
                quote!(v),
            ),
            Kind::Opt(i) => (
                quote!(<__X: #air::__rt::IntoOpt<#i, __M>, __M>),
                quote!(f: impl ::core::ops::FnOnce() -> __X),
                quote!(#air::__rt::IntoOpt::into_opt(#air::untrack(f))),
                quote!(<__X: #air::__rt::IntoOpt<#i, __M>, __M>),
                quote!(v: __X),
                quote!(#air::__rt::IntoOpt::into_opt(v)),
            ),
            Kind::Prop(i, opt) => {
                let (tr, call) = if *opt {
                    (quote!(#air::__rt::IntoOpt<#i, __M>), quote!(#air::__rt::IntoOpt::into_opt(f())))
                } else {
                    (quote!(#air::__rt::IntoValue<#i, __M>), quote!(#air::__rt::IntoValue::into_value(f())))
                };
                (
                    quote!(<__X: #tr + 'static, __M>),
                    quote!(f: impl ::core::ops::Fn() -> __X + 'static),
                    quote!(#air::Prop::derive(::std::boxed::Box::new(move || #call))),
                    quote!(<__X: #tr, __M>),
                    quote!(v: __X),
                    quote!(#air::Prop::Const({ let f = || v; #call })),
                )
            }
            // A view or text: the closure itself, so text that reads state follows it.
            Kind::View => (
                quote!(<__F: #air::__rt::IntoView<__M>, __M>),
                quote!(f: __F),
                quote!(#air::__rt::IntoView::into_view(f)),
                quote!(<__X: #air::__rt::IntoViewValue<__M>, __M>),
                quote!(v: __X),
                quote!(#air::__rt::IntoViewValue::into_view_value(v)),
            ),
            Kind::OptView => (
                quote!(<__F: #air::__rt::IntoOptView<__M>, __M>),
                quote!(f: __F),
                quote!(#air::__rt::IntoOptView::into_opt_view(f)),
                quote!(<__X: #air::__rt::IntoOptViewValue<__M>, __M>),
                quote!(v: __X),
                quote!(#air::__rt::IntoOptViewValue::into_opt_view_value(v)),
            ),
            Kind::Plain => (
                quote!(<__X: ::core::convert::Into<#ty>>),
                quote!(f: impl ::core::ops::FnOnce() -> __X),
                quote!(::core::convert::Into::into(#air::untrack(f))),
                quote!(<__X: ::core::convert::Into<#ty>>),
                quote!(v: __X),
                quote!(::core::convert::Into::into(v)),
            ),
        };
        let (fgen, fparam, fval) = if cb {
            let mk = quote!(#air::Callback::new(f));
            (
                quote!(),
                quote!(f: impl ::core::ops::FnMut(<#ty as #air::__rt::CallbackOf>::A) -> <#ty as #air::__rt::CallbackOf>::R + 'static),
                if opt_cb { quote!(::core::option::Option::Some(#mk)) } else { mk },
            )
        } else {
            (fgen, fparam, fval)
        };
        // `__val_n(v)`: a literal number, `true`, `Variant::A`: the type is the prop's.
        let vname = format_ident!("__val_{}", n.to_string().trim_start_matches("r#"));
        let (vty, vval) = match &f.kind {
            Kind::Opt(i) => (i.clone(), quote!(::core::option::Option::Some(v))),
            Kind::Prop(i, false) => (i.clone(), quote!(#air::Prop::Const(v))),
            Kind::Prop(i, true) => (i.clone(), quote!(#air::Prop::Const(::core::option::Option::Some(v)))),
            Kind::OptView => (quote!(#air::View), quote!(::core::option::Option::Some(v))),
            Kind::Plain | Kind::View => (ty.clone(), quote!(v)),
        };
        // A view prop takes text as a value too (`label: 7`).
        let (vgen, vparam, vval) = match &f.kind {
            Kind::View | Kind::OptView if !exact => (fgen.clone(), fparam.clone(), fval.clone()),
            _ => (quote!(), quote!(v: #vty), vval),
        };
        let docs = &f.docs;
        match required.iter().position(|&r| r == k) {
            None => (
                quote!(
                    #(#docs)*
                    pub fn #n #gens (mut self, #param) -> Self { self.#n = ::core::option::Option::Some(#val); self }
                    #[doc(hidden)]
                    pub fn #fname #fgen (mut self, #fparam) -> Self { self.#n = ::core::option::Option::Some(#fval); self }
                    #[doc(hidden)]
                    pub fn #vname #vgen (mut self, #vparam) -> Self { self.#n = ::core::option::Option::Some(#vval); self }
                ),
                quote!(),
            ),
            Some(r) => {
                let others: Vec<&Ident> = params.iter().enumerate().filter(|(j, _)| *j != r).map(|(_, p)| p).collect();
                let before: Vec<TokenStream> =
                    params.iter().enumerate().map(|(j, p)| if j == r { quote!(()) } else { quote!(#p) }).collect();
                let after: Vec<TokenStream> = params
                    .iter()
                    .enumerate()
                    .map(|(j, p)| if j == r { quote!(#air::__rt::Set) } else { quote!(#p) })
                    .collect();
                let rest: Vec<&Ident> = names.iter().copied().filter(|m| *m != n).collect();
                let mk = |v: &TokenStream| quote!(#b { #n: ::core::option::Option::Some(#v), #(#rest: self.#rest,)* __s: ::core::marker::PhantomData });
                let (mv, mfv, mvv) = (mk(&val), mk(&fval), mk(&vval));
                (
                    quote!(),
                    quote!(
                        #[allow(non_camel_case_types)]
                        impl<#gp #(#others),*> #b<#gn #(#before),*> {
                            #(#docs)*
                            pub fn #n #gens (self, #param) -> #b<#gn #(#after),*> { #mv }
                            #[doc(hidden)]
                            pub fn #fname #fgen (self, #fparam) -> #b<#gn #(#after),*> { #mfv }
                            #[doc(hidden)]
                            pub fn #vname #vgen (self, #vparam) -> #b<#gn #(#after),*> { #mvv }
                        }
                    ),
                )
            }
        }
    };
    let (mut optional, mut req_impls) = (vec![], vec![]);
    for k in 0..fields.len() {
        let (o, r) = setter(k);
        optional.push(o);
        req_impls.push(r);
    }
    let needs: Vec<Ident> = required
        .iter()
        .map(|&k| format_ident!("__{}_needs_prop_{}", shown, fields[k].name.to_string().trim_start_matches("r#")))
        .collect();
    let need_msgs = required
        .iter()
        .map(|&k| format!("`{shown}` needs its prop `{}`", fields[k].name.to_string().trim_start_matches("r#")));
    let need_labels =
        required.iter().map(|&k| format!("add `{}: …` here", fields[k].name.to_string().trim_start_matches("r#")));
    let fill = fields.iter().map(|f| {
        let n = &f.name;
        let or = match (&f.default, &f.kind) {
            (Some(d), Kind::Prop(..)) => quote!(#air::Prop::Const(#d)),
            (Some(d), _) => d.clone(),
            (None, Kind::Opt(_) | Kind::OptView) => quote!(::core::option::Option::None),
            (None, Kind::Prop(_, true)) => quote!(#air::Prop::Const(::core::option::Option::None)),
            (None, Kind::View) => quote!(::core::result::Result::Ok(#air::Tree::empty())),
            (None, _) if f.extends => quote!(::std::vec::Vec::new()),
            (None, _) => quote!(::core::unreachable!()),
        };
        quote!(#n: match self.#n { ::core::option::Option::Some(v) => v, ::core::option::Option::None => #or })
    });
    let attrs = fields.iter().find(|f| f.extends).map(|f| {
        let n = &f.name;
        quote!(
            #[allow(non_camel_case_types)]
            impl<#gp #(#params),*> #air::__rt::GlobalAttributes for #b<#gn #(#params),*> {
                fn __push(&mut self, a: #air::Attribute) {
                    self.#n.get_or_insert_with(::std::vec::Vec::new).push(a)
                }
            }
        )
    });
    quote!(
        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        #vis struct #b<#gp #(#params = ()),*> {
            #(#names: ::core::option::Option<#tys>,)*
            __s: ::core::marker::PhantomData<(#(#params,)*)>,
        }
        impl<#gp> #air::__rt::Props for #props<#gn> {
            type Builder = #b<#gn #(#unit),*>;
            fn builder() -> Self::Builder {
                #b { #(#names: ::core::option::Option::None,)* __s: ::core::marker::PhantomData }
            }
        }
        #[allow(non_camel_case_types)]
        impl<#gp #(#params),*> #b<#gn #(#params),*> {
            #(#optional)*
        }
        #(#req_impls)*
        #(
            #[doc(hidden)]
            #[allow(non_camel_case_types)]
            #[diagnostic::on_unimplemented(message = #need_msgs, label = #need_labels)]
            #vis trait #needs {}
            impl #needs for #air::__rt::Set {}
        )*
        #[allow(non_camel_case_types)]
        impl<#gp #(#params: #needs),*> #b<#gn #(#params),*> {
            pub fn build(self) -> #props<#gn> {
                #props { #(#fill,)* }
            }
        }
        #attrs
    )
}

type Res<T> = Result<T, (Span, String)>;

/// A `match` arm in `rsx!`: its pattern, its guard, its nodes.
pub(crate) type Arm = (TokenStream, Option<TokenStream>, Vec<Node>);

pub(crate) enum Node {
    Text(String),
    /// A `"…{x}…"` text: `format!`'s literal and arguments, and the names it reads.
    Fmt(TokenStream, Vec<String>),
    Expr(TokenStream),
    El {
        tag: String,
        /// Where its tag is written (`ax lint`'s line).
        #[cfg_attr(proc_macro, allow(dead_code))]
        at: Span,
        attrs: Vec<(String, Val)>,
        spreads: Vec<TokenStream>,
        children: Vec<Node>,
        key: Option<TokenStream>,
    },
    Comp {
        path: Vec<TokenTree>,
        props: Vec<(PropName, TokenStream)>,
        spreads: Vec<TokenStream>,
        children: Vec<Node>,
        key: Option<TokenStream>,
    },
    For {
        pat: TokenStream,
        expr: TokenStream,
        body: Vec<Node>,
    },
    /// `match expr { pat if guard => { nodes } … }`: each arm its pattern, guard and nodes.
    Match {
        expr: TokenStream,
        arms: Vec<Arm>,
    },
    If {
        cond: TokenStream,
        then: Vec<Node>,
        otherwise: Vec<Node>,
    },
}

/// A component's prop: a declared one (a name), or an attribute (`"data-t"`).
pub(crate) enum PropName {
    Field(Ident),
    Attr(String),
}

pub(crate) enum Val {
    Static(String),
    Present(bool),
    Fmt(TokenStream, Vec<String>),
    Event(String, TokenStream),
    Expr(TokenStream),
}

fn is_ident(t: &TokenTree, s: &str) -> bool {
    matches!(t, TokenTree::Ident(i) if i == s)
}
fn is_punct(t: Option<&TokenTree>, c: char) -> bool {
    matches!(t, Some(TokenTree::Punct(p)) if p.as_char() == c)
}
/// `name:` (one colon, not a path's `::`).
fn is_colon(toks: &[TokenTree], i: usize) -> bool {
    matches!(toks.get(i), Some(TokenTree::Punct(p)) if p.as_char() == ':' && p.spacing() == Spacing::Alone)
        && !is_punct(toks.get(i.wrapping_sub(1)), ':')
}
fn span_of(toks: &[TokenTree], i: usize) -> Span {
    toks.get(i).or(toks.last()).map_or(Span::call_site(), |t| t.span())
}

fn parse_nodes(toks: &[TokenTree]) -> Res<Vec<Node>> {
    let (mut out, mut i) = (vec![], 0);
    while i < toks.len() {
        if is_punct(toks.get(i), ',') {
            i += 1;
            continue;
        }
        out.push(parse_node(toks, &mut i)?);
    }
    Ok(out)
}

/// Tokens until the first top-level `{ … }` (Rust's no-struct-literal
/// expression, as after `for … in` and `if`), and that block.
fn until_block(toks: &[TokenTree], i: &mut usize) -> Res<(TokenStream, Vec<TokenTree>)> {
    let start = *i;
    while *i < toks.len() {
        if let TokenTree::Group(g) = &toks[*i]
            && g.delimiter() == Delimiter::Brace
        {
            *i += 1;
            return Ok((toks[start..*i - 1].iter().cloned().collect(), g.stream().into_iter().collect()));
        }
        *i += 1;
    }
    Err((span_of(toks, start), "air: expected a `{ … }` block".into()))
}

fn parse_node(toks: &[TokenTree], i: &mut usize) -> Res<Node> {
    let t = &toks[*i];
    if let TokenTree::Literal(l) = t
        && let Some(s) = lit_str(l)
    {
        *i += 1;
        return Ok(text(&s, l.span()));
    }
    if let TokenTree::Group(g) = t
        && g.delimiter() == Delimiter::Brace
    {
        *i += 1;
        return Ok(Node::Expr(g.stream()));
    }
    if is_ident(t, "for") {
        *i += 1;
        let start = *i;
        while *i < toks.len() && !is_ident(&toks[*i], "in") {
            *i += 1;
        }
        let pat: TokenStream = toks[start..*i].iter().cloned().collect();
        *i += 1;
        let (expr, body) = until_block(toks, i)?;
        return Ok(Node::For { pat, expr, body: parse_nodes(&body)? });
    }
    if is_ident(t, "if") {
        *i += 1;
        let (cond, then) = until_block(toks, i)?;
        let mut otherwise = vec![];
        if toks.get(*i).is_some_and(|t| is_ident(t, "else")) {
            *i += 1;
            if toks.get(*i).is_some_and(|t| is_ident(t, "if")) {
                otherwise.push(parse_node(toks, i)?);
            } else {
                otherwise = parse_nodes(&until_block(toks, i)?.1)?;
            }
        }
        return Ok(Node::If { cond, then: parse_nodes(&then)?, otherwise });
    }
    if is_ident(t, "match") {
        *i += 1;
        let (expr, body) = until_block(toks, i)?;
        return Ok(Node::Match { expr, arms: arms(&body)? });
    }
    if is_ident(t, "while") || is_ident(t, "loop") {
        return Err((
            t.span(),
            format!("air: `{t}` in rsx!: a view says what shows, not how long to repeat: a `for` over the items"),
        ));
    }
    if let TokenTree::Ident(_) = t {
        let start = *i;
        let (path, inner) = until_block(toks, i)?;
        let path: Vec<TokenTree> = path.into_iter().collect();
        let last = path.iter().rev().find_map(|t| match t {
            TokenTree::Ident(i) => Some(i.to_string()),
            _ => None,
        });
        let comp = path.len() > 1 || last.as_ref().is_some_and(|s| s.starts_with(|c: char| c.is_uppercase()));
        // A component's turbofish (`WebIsland::<Chart, ()>`) may hold any type.
        let turbofish = path.iter().any(|t| is_punct(Some(t), '<'));
        if path.is_empty()
            || (!turbofish && path.iter().any(|t| !matches!(t, TokenTree::Ident(_) | TokenTree::Punct(_))))
        {
            return Err((span_of(toks, start), "air: expected an element or a component".into()));
        }
        return element(path, &inner, comp);
    }
    Err((t.span(), "air: unexpected in rsx!".into()))
}

/// A `match`'s arms: `pat [if guard] => { nodes }` or `=> node`, `,` between.
fn arms(toks: &[TokenTree]) -> Res<Vec<Arm>> {
    let (mut out, mut i) = (vec![], 0);
    while i < toks.len() {
        let start = i;
        while i < toks.len() && !(is_punct(toks.get(i), '=') && is_punct(toks.get(i + 1), '>')) {
            i += 1;
        }
        if i == toks.len() {
            return Err((span_of(toks, start), "air: a `match` arm in rsx! is `pattern => { nodes }`".into()));
        }
        let head = &toks[start..i];
        let (pat, guard) = match head.iter().position(|t| is_ident(t, "if")) {
            Some(g) => (head[..g].iter().cloned().collect(), Some(head[g + 1..].iter().cloned().collect())),
            None => (head.iter().cloned().collect(), None),
        };
        i += 2;
        let nodes = match toks.get(i) {
            Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
                i += 1;
                parse_nodes(&g.stream().into_iter().collect::<Vec<_>>())?
            }
            Some(_) => vec![parse_node(toks, &mut i)?],
            None => return Err((span_of(toks, start), "air: a `match` arm in rsx! is `pattern => { nodes }`".into())),
        };
        if is_punct(toks.get(i), ',') {
            i += 1;
        }
        out.push((pat, guard, nodes));
    }
    Ok(out)
}

/// An element's or component's `{ … }`: `name: value,` pairs, then children.
fn element(path: Vec<TokenTree>, toks: &[TokenTree], comp: bool) -> Res<Node> {
    let (mut attrs, mut props, mut spreads, mut children, mut key, mut i) = (vec![], vec![], vec![], vec![], None, 0);
    while i < toks.len() {
        if is_punct(toks.get(i), ',') {
            i += 1;
            continue;
        }
        if is_punct(toks.get(i), '.') && is_punct(toks.get(i + 1), '.') {
            i += 2;
            spreads.push(take_expr(toks, &mut i));
            continue;
        }
        let named = matches!(&toks[i], TokenTree::Ident(_) | TokenTree::Literal(_)) && is_colon(toks, i + 1);
        // `class,` / a component's `name,`: the name is the value.
        let short = matches!(&toks[i], TokenTree::Ident(_)) && (i + 1 == toks.len() || is_punct(toks.get(i + 1), ','));
        if !named && !short {
            children.push(parse_node(toks, &mut i)?);
            continue;
        }
        let name_tok = toks[i].clone();
        let value: TokenStream = if short {
            i += 1;
            name_tok.clone().into()
        } else {
            i += 2;
            take_expr(toks, &mut i)
        };
        let name = match &name_tok {
            TokenTree::Literal(l) => {
                lit_str(l).ok_or((l.span(), "air: an attribute name is a string or a name".to_string()))?
            }
            t => t.to_string().trim_start_matches("r#").to_string(),
        };
        if name == "key" {
            key = Some(match value.clone().into_iter().next() {
                Some(TokenTree::Literal(l)) if lit_str(&l).is_some() => match text(&lit_str(&l).unwrap(), l.span()) {
                    Node::Fmt(f, _) => quote!(::std::format!(#f)),
                    _ => value,
                },
                _ => value,
            });
        } else if comp {
            props.push(match name_tok {
                TokenTree::Ident(id) => (PropName::Field(id), value),
                _ => (PropName::Attr(name), value),
            });
        } else {
            if let Some(ev) = name.strip_prefix("on")
                && payload(ev).is_none()
            {
                return Err((
                    name_tok.span(),
                    format!("air: no event `{name}` (a DOM event's name, as in `onclick`, `onkeydown`, `oninput`)"),
                ));
            }
            // A style attribute: its page name says so (`style:align-items`).
            let style = match (&name_tok, path.first()) {
                (TokenTree::Ident(_), Some(t)) => style_attr(t.to_string().trim_start_matches("r#"), &name),
                _ => None,
            };
            let page_name = style.map_or_else(|| attr_name(&name), |css| format!("style:{css}"));
            attrs.push((page_name, attr_val(&name, value)));
        }
    }
    if comp {
        return Ok(Node::Comp { path, props, spreads, children, key });
    }
    let (tag, at) = (path[0].to_string().trim_start_matches("r#").to_string(), path[0].span());
    Ok(Node::El { tag, at, attrs, spreads, children, key })
}

/// The style attributes (the HTML global attributes' `style` namespace, as
/// aio-rs 0.1's views took them): on an HTML element, `align_items:
/// "center"` sets that CSS property; sorted by name.
const STYLE_ATTRS: &[(&str, &str)] = &[
    ("align_content", "align-content"),
    ("align_items", "align-items"),
    ("align_self", "align-self"),
    ("alignment_adjust", "alignment-adjust"),
    ("alignment_baseline", "alignment-baseline"),
    ("all", "all"),
    ("alt", "alt"),
    ("animation", "animation"),
    ("animation_delay", "animation-delay"),
    ("animation_direction", "animation-direction"),
    ("animation_duration", "animation-duration"),
    ("animation_fill_mode", "animation-fill-mode"),
    ("animation_iteration_count", "animation-iteration-count"),
    ("animation_name", "animation-name"),
    ("animation_play_state", "animation-play-state"),
    ("animation_timing_function", "animation-timing-function"),
    ("aspect_ratio", "aspect-ratio"),
    ("azimuth", "azimuth"),
    ("backdrop_filter", "backdrop-filter"),
    ("backface_visibility", "backface-visibility"),
    ("background", "background"),
    ("background_attachment", "background-attachment"),
    ("background_blend_mode", "background-blend-mode"),
    ("background_clip", "background-clip"),
    ("background_color", "background-color"),
    ("background_image", "background-image"),
    ("background_origin", "background-origin"),
    ("background_position", "background-position"),
    ("background_repeat", "background-repeat"),
    ("background_size", "background-size"),
    ("baseline_shift", "baseline-shift"),
    ("bleed", "bleed"),
    ("bookmark_label", "bookmark-label"),
    ("bookmark_level", "bookmark-level"),
    ("bookmark_state", "bookmark-state"),
    ("border", "border"),
    ("border_bottom", "border-bottom"),
    ("border_bottom_color", "border-bottom-color"),
    ("border_bottom_left_radius", "border-bottom-left-radius"),
    ("border_bottom_right_radius", "border-bottom-right-radius"),
    ("border_bottom_style", "border-bottom-style"),
    ("border_bottom_width", "border-bottom-width"),
    ("border_collapse", "border-collapse"),
    ("border_color", "border-color"),
    ("border_image", "border-image"),
    ("border_image_outset", "border-image-outset"),
    ("border_image_repeat", "border-image-repeat"),
    ("border_image_slice", "border-image-slice"),
    ("border_image_source", "border-image-source"),
    ("border_image_width", "border-image-width"),
    ("border_left", "border-left"),
    ("border_left_color", "border-left-color"),
    ("border_left_style", "border-left-style"),
    ("border_left_width", "border-left-width"),
    ("border_radius", "border-radius"),
    ("border_right", "border-right"),
    ("border_right_color", "border-right-color"),
    ("border_right_style", "border-right-style"),
    ("border_right_width", "border-right-width"),
    ("border_spacing", "border-spacing"),
    ("border_style", "border-style"),
    ("border_top", "border-top"),
    ("border_top_color", "border-top-color"),
    ("border_top_left_radius", "border-top-left-radius"),
    ("border_top_right_radius", "border-top-right-radius"),
    ("border_top_style", "border-top-style"),
    ("border_top_width", "border-top-width"),
    ("border_width", "border-width"),
    ("bottom", "bottom"),
    ("box_decoration_break", "box-decoration-break"),
    ("box_shadow", "box-shadow"),
    ("box_sizing", "box-sizing"),
    ("box_snap", "box-snap"),
    ("break_after", "break-after"),
    ("break_before", "break-before"),
    ("break_inside", "break-inside"),
    ("buffered_rendering", "buffered-rendering"),
    ("caption_side", "caption-side"),
    ("clear", "clear"),
    ("clear_side", "clear-side"),
    ("clip", "clip"),
    ("clip_path", "clip-path"),
    ("clip_rule", "clip-rule"),
    ("color", "color"),
    ("color_adjust", "color-adjust"),
    ("color_correction", "color-correction"),
    ("color_interpolation", "color-interpolation"),
    ("color_interpolation_filters", "color-interpolation-filters"),
    ("color_profile", "color-profile"),
    ("color_rendering", "color-rendering"),
    ("column_count", "column-count"),
    ("column_fill", "column-fill"),
    ("column_gap", "column-gap"),
    ("column_rule", "column-rule"),
    ("column_rule_color", "column-rule-color"),
    ("column_rule_style", "column-rule-style"),
    ("column_rule_width", "column-rule-width"),
    ("column_span", "column-span"),
    ("column_width", "column-width"),
    ("columns", "columns"),
    ("contain", "contain"),
    ("content", "content"),
    ("counter_increment", "counter-increment"),
    ("counter_reset", "counter-reset"),
    ("counter_set", "counter-set"),
    ("cue", "cue"),
    ("cue_after", "cue-after"),
    ("cue_before", "cue-before"),
    ("cursor", "cursor"),
    ("direction", "direction"),
    ("display", "display"),
    ("display_box", "display-box"),
    ("display_extras", "display-extras"),
    ("display_inside", "display-inside"),
    ("display_outside", "display-outside"),
    ("dominant_baseline", "dominant-baseline"),
    ("elevation", "elevation"),
    ("empty_cells", "empty-cells"),
    ("enable_background", "enable-background"),
    ("fill", "fill"),
    ("fill_opacity", "fill-opacity"),
    ("fill_rule", "fill-rule"),
    ("filter", "filter"),
    ("flex", "flex"),
    ("flex_basis", "flex-basis"),
    ("flex_direction", "flex-direction"),
    ("flex_flow", "flex-flow"),
    ("flex_grow", "flex-grow"),
    ("flex_shrink", "flex-shrink"),
    ("flex_wrap", "flex-wrap"),
    ("float", "float"),
    ("float_defer_column", "float-defer-column"),
    ("float_defer_page", "float-defer-page"),
    ("float_offset", "float-offset"),
    ("float_wrap", "float-wrap"),
    ("flood_color", "flood-color"),
    ("flood_opacity", "flood-opacity"),
    ("flow_from", "flow-from"),
    ("flow_into", "flow-into"),
    ("font", "font"),
    ("font_family", "font-family"),
    ("font_feature_settings", "font-feature-settings"),
    ("font_kerning", "font-kerning"),
    ("font_language_override", "font-language-override"),
    ("font_size", "font-size"),
    ("font_size_adjust", "font-size-adjust"),
    ("font_stretch", "font-stretch"),
    ("font_style", "font-style"),
    ("font_synthesis", "font-synthesis"),
    ("font_variant", "font-variant"),
    ("font_variant_alternates", "font-variant-alternates"),
    ("font_variant_caps", "font-variant-caps"),
    ("font_variant_east_asian", "font-variant-east-asian"),
    ("font_variant_ligatures", "font-variant-ligatures"),
    ("font_variant_numeric", "font-variant-numeric"),
    ("font_variant_position", "font-variant-position"),
    ("font_weight", "font-weight"),
    ("footnote_policy", "footnote-policy"),
    ("gap", "gap"),
    ("glyph_orientation_horizontal", "glyph-orientation-horizontal"),
    ("glyph_orientation_vertical", "glyph-orientation-vertical"),
    ("grid", "grid"),
    ("grid_area", "grid-area"),
    ("grid_auto_columns", "grid-auto-columns"),
    ("grid_auto_flow", "grid-auto-flow"),
    ("grid_auto_rows", "grid-auto-rows"),
    ("grid_column", "grid-column"),
    ("grid_column_end", "grid-column-end"),
    ("grid_column_start", "grid-column-start"),
    ("grid_row", "grid-row"),
    ("grid_row_end", "grid-row-end"),
    ("grid_row_start", "grid-row-start"),
    ("grid_template", "grid-template"),
    ("grid_template_areas", "grid-template-areas"),
    ("grid_template_columns", "grid-template-columns"),
    ("grid_template_rows", "grid-template-rows"),
    ("hanging_punctuation", "hanging-punctuation"),
    ("height", "height"),
    ("hyphenate_character", "hyphenate-character"),
    ("hyphenate_limit_chars", "hyphenate-limit-chars"),
    ("hyphenate_limit_last", "hyphenate-limit-last"),
    ("hyphenate_limit_lines", "hyphenate-limit-lines"),
    ("hyphenate_limit_zone", "hyphenate-limit-zone"),
    ("hyphens", "hyphens"),
    ("icon", "icon"),
    ("image_orientation", "image-orientation"),
    ("image_rendering", "image-rendering"),
    ("image_resolution", "image-resolution"),
    ("ime", "ime"),
    ("ime_align", "ime-align"),
    ("ime_mode", "ime-mode"),
    ("ime_offset", "ime-offset"),
    ("ime_width", "ime-width"),
    ("initial_letters", "initial-letters"),
    ("inline_box_align", "inline-box-align"),
    ("isolation", "isolation"),
    ("justify_content", "justify-content"),
    ("justify_items", "justify-items"),
    ("justify_self", "justify-self"),
    ("kerning", "kerning"),
    ("left", "left"),
    ("letter_spacing", "letter-spacing"),
    ("lighting_color", "lighting-color"),
    ("line_box_contain", "line-box-contain"),
    ("line_break", "line-break"),
    ("line_grid", "line-grid"),
    ("line_height", "line-height"),
    ("line_slack", "line-slack"),
    ("line_snap", "line-snap"),
    ("list_style", "list-style"),
    ("list_style_image", "list-style-image"),
    ("list_style_position", "list-style-position"),
    ("list_style_type", "list-style-type"),
    ("list_styler_type", "list-style-type"),
    ("margin", "margin"),
    ("margin_bottom", "margin-bottom"),
    ("margin_left", "margin-left"),
    ("margin_right", "margin-right"),
    ("margin_top", "margin-top"),
    ("marker", "marker"),
    ("marker_end", "marker-end"),
    ("marker_knockout_left", "marker-knockout-left"),
    ("marker_knockout_right", "marker-knockout-right"),
    ("marker_mid", "marker-mid"),
    ("marker_pattern", "marker-pattern"),
    ("marker_segment", "marker-segment"),
    ("marker_side", "marker-side"),
    ("marker_start", "marker-start"),
    ("marks", "marks"),
    ("marquee_direction", "marquee-direction"),
    ("marquee_play_count", "marquee-play-count"),
    ("marquee_speed", "marquee-speed"),
    ("marquee_style", "marquee-style"),
    ("mask", "mask"),
    ("mask_box", "mask-box"),
    ("mask_box_outset", "mask-box-outset"),
    ("mask_box_repeat", "mask-box-repeat"),
    ("mask_box_slice", "mask-box-slice"),
    ("mask_box_source", "mask-box-source"),
    ("mask_box_width", "mask-box-width"),
    ("mask_clip", "mask-clip"),
    ("mask_image", "mask-image"),
    ("mask_origin", "mask-origin"),
    ("mask_position", "mask-position"),
    ("mask_repeat", "mask-repeat"),
    ("mask_size", "mask-size"),
    ("mask_type", "mask-type"),
    ("max_height", "max-height"),
    ("max_lines", "max-lines"),
    ("max_width", "max-width"),
    ("min_height", "min-height"),
    ("min_width", "min-width"),
    ("mix_blend_mode", "mix-blend-mode"),
    ("nav_down", "nav-down"),
    ("nav_index", "nav-index"),
    ("nav_left", "nav-left"),
    ("nav_right", "nav-right"),
    ("nav_up", "nav-up"),
    ("object_fit", "object-fit"),
    ("object_position", "object-position"),
    ("offset_after", "offset-after"),
    ("offset_before", "offset-before"),
    ("offset_end", "offset-end"),
    ("offset_start", "offset-start"),
    ("opacity", "opacity"),
    ("order", "order"),
    ("orphans", "orphans"),
    ("outline", "outline"),
    ("outline_color", "outline-color"),
    ("outline_offset", "outline-offset"),
    ("outline_style", "outline-style"),
    ("outline_width", "outline-width"),
    ("overflow", "overflow"),
    ("overflow_style", "overflow-style"),
    ("overflow_wrap", "overflow-wrap"),
    ("overflow_x", "overflow-x"),
    ("overflow_y", "overflow-y"),
    ("padding", "padding"),
    ("padding_bottom", "padding-bottom"),
    ("padding_left", "padding-left"),
    ("padding_right", "padding-right"),
    ("padding_top", "padding-top"),
    ("page", "page"),
    ("page_break_after", "page-break-after"),
    ("page_break_before", "page-break-before"),
    ("page_break_inside", "page-break-inside"),
    ("paint_order", "paint-order"),
    ("pause", "pause"),
    ("pause_after", "pause-after"),
    ("pause_before", "pause-before"),
    ("perspective", "perspective"),
    ("perspective_origin", "perspective-origin"),
    ("pitch", "pitch"),
    ("pitch_range", "pitch-range"),
    ("play_during", "play-during"),
    ("pointer_events", "pointer-events"),
    ("position", "position"),
    ("quotes", "quotes"),
    ("region_fragment", "region-fragment"),
    ("resize", "resize"),
    ("rest", "rest"),
    ("rest_after", "rest-after"),
    ("rest_before", "rest-before"),
    ("richness", "richness"),
    ("right", "right"),
    ("row_gap", "row-gap"),
    ("ruby_align", "ruby-align"),
    ("ruby_merge", "ruby-merge"),
    ("ruby_position", "ruby-position"),
    ("scroll_behavior", "scroll-behavior"),
    ("scroll_snap_coordinate", "scroll-snap-coordinate"),
    ("scroll_snap_destination", "scroll-snap-destination"),
    ("scroll_snap_points_x", "scroll-snap-points-x"),
    ("scroll_snap_points_y", "scroll-snap-points-y"),
    ("scroll_snap_type", "scroll-snap-type"),
    ("shape_image_threshold", "shape-image-threshold"),
    ("shape_inside", "shape-inside"),
    ("shape_margin", "shape-margin"),
    ("shape_outside", "shape-outside"),
    ("shape_padding", "shape-padding"),
    ("shape_rendering", "shape-rendering"),
    ("size", "size"),
    ("speak", "speak"),
    ("speak_as", "speak-as"),
    ("speak_header", "speak-header"),
    ("speak_numeral", "speak-numeral"),
    ("speak_punctuation", "speak-punctuation"),
    ("speech_rate", "speech-rate"),
    ("stop_color", "stop-color"),
    ("stop_opacity", "stop-opacity"),
    ("stress", "stress"),
    ("string_set", "string-set"),
    ("stroke", "stroke"),
    ("stroke_dasharray", "stroke-dasharray"),
    ("stroke_dashoffset", "stroke-dashoffset"),
    ("stroke_linecap", "stroke-linecap"),
    ("stroke_linejoin", "stroke-linejoin"),
    ("stroke_miterlimit", "stroke-miterlimit"),
    ("stroke_opacity", "stroke-opacity"),
    ("stroke_width", "stroke-width"),
    ("tab_size", "tab-size"),
    ("table_layout", "table-layout"),
    ("text_align", "text-align"),
    ("text_align_all", "text-align-all"),
    ("text_align_last", "text-align-last"),
    ("text_anchor", "text-anchor"),
    ("text_combine_upright", "text-combine-upright"),
    ("text_decoration", "text-decoration"),
    ("text_decoration_color", "text-decoration-color"),
    ("text_decoration_line", "text-decoration-line"),
    ("text_decoration_skip", "text-decoration-skip"),
    ("text_decoration_style", "text-decoration-style"),
    ("text_emphasis", "text-emphasis"),
    ("text_emphasis_color", "text-emphasis-color"),
    ("text_emphasis_position", "text-emphasis-position"),
    ("text_emphasis_skip", "text-emphasis-skip"),
    ("text_emphasis_style", "text-emphasis-style"),
    ("text_height", "text-height"),
    ("text_indent", "text-indent"),
    ("text_justify", "text-justify"),
    ("text_orientation", "text-orientation"),
    ("text_overflow", "text-overflow"),
    ("text_rendering", "text-rendering"),
    ("text_shadow", "text-shadow"),
    ("text_size_adjust", "text-size-adjust"),
    ("text_space_collapse", "text-space-collapse"),
    ("text_spacing", "text-spacing"),
    ("text_transform", "text-transform"),
    ("text_underline_position", "text-underline-position"),
    ("text_wrap", "text-wrap"),
    ("top", "top"),
    ("touch_action", "touch-action"),
    ("transform", "transform"),
    ("transform_box", "transform-box"),
    ("transform_origin", "transform-origin"),
    ("transform_style", "transform-style"),
    ("transition", "transition"),
    ("transition_delay", "transition-delay"),
    ("transition_duration", "transition-duration"),
    ("transition_property", "transition-property"),
    ("transition_timing_function", "transition-timing-function"),
    ("unicode_bidi", "unicode-bidi"),
    ("user_select", "user-select"),
    ("vector_effect", "vector-effect"),
    ("vertical_align", "vertical-align"),
    ("visibility", "visibility"),
    ("voice_balance", "voice-balance"),
    ("voice_duration", "voice-duration"),
    ("voice_family", "voice-family"),
    ("voice_pitch", "voice-pitch"),
    ("voice_range", "voice-range"),
    ("voice_rate", "voice-rate"),
    ("voice_stress", "voice-stress"),
    ("voice_volume", "voice-volume"),
    ("volume", "volume"),
    ("webkit_user_select", "-webkit-user-select"),
    ("white_space", "white-space"),
    ("widows", "widows"),
    ("width", "width"),
    ("will_change", "will-change"),
    ("word_break", "word-break"),
    ("word_spacing", "word-spacing"),
    ("word_wrap", "word-wrap"),
    ("wrap_flow", "wrap-flow"),
    ("wrap_through", "wrap-through"),
    ("writing_mode", "writing-mode"),
    ("z_index", "z-index"),
];

/// An HTML element's own attributes that share a style attribute's name
/// (the HTML elements'): `img { width }` is the attribute, `div { width }` the CSS property.
const OWN_ATTRS: &[(&str, &[&str])] = &[
    ("canvas", &["height", "width"]),
    ("embed", &["height", "width"]),
    ("iframe", &["height", "width"]),
    ("img", &["alt", "height", "width"]),
    ("input", &["alt", "height", "size", "width"]),
    ("meta", &["content"]),
    ("object", &["height", "width"]),
    ("select", &["size"]),
    ("u", &["alt"]),
    ("video", &["height", "width"]),
];

/// The CSS property a named attribute of HTML element `tag` sets, if it is
/// one of the style attributes (`align_items` → `align-items`): set
/// with `style.setProperty`; a quoted name never is.
fn style_attr(tag: &str, name: &str) -> Option<&'static str> {
    if tag == "svg" || SVG_ONLY.contains(&tag) || OWN_ATTRS.iter().any(|(t, a)| *t == tag && a.contains(&name)) {
        return None;
    }
    STYLE_ATTRS.binary_search_by(|(k, _)| k.cmp(&name)).ok().map(|i| STYLE_ATTRS[i].1)
}

/// An attribute's name in the page: `aria_label` is `aria-label`, SVG's
/// camelCase names keep their case, `dangerous_inner_html` is the
/// `innerHTML` property.
fn attr_name(name: &str) -> String {
    const CAMEL: &[(&str, &str)] = &[
        ("view_box", "viewBox"),
        ("preserve_aspect_ratio", "preserveAspectRatio"),
        ("gradient_units", "gradientUnits"),
        ("gradient_transform", "gradientTransform"),
        ("pattern_units", "patternUnits"),
        ("pattern_content_units", "patternContentUnits"),
        ("pattern_transform", "patternTransform"),
        ("marker_width", "markerWidth"),
        ("marker_height", "markerHeight"),
        ("marker_units", "markerUnits"),
        ("ref_x", "refX"),
        ("ref_y", "refY"),
        ("text_length", "textLength"),
        ("length_adjust", "lengthAdjust"),
        ("spread_method", "spreadMethod"),
        ("std_deviation", "stdDeviation"),
        ("clip_path_units", "clipPathUnits"),
        ("mask_units", "maskUnits"),
        ("mask_content_units", "maskContentUnits"),
        ("path_length", "pathLength"),
        ("filter_units", "filterUnits"),
        ("primitive_units", "primitiveUnits"),
        ("start_offset", "startOffset"),
        ("base_frequency", "baseFrequency"),
        ("num_octaves", "numOctaves"),
        ("stitch_tiles", "stitchTiles"),
        ("dangerous_inner_html", "innerHTML"),
    ];
    CAMEL.iter().find(|(k, _)| *k == name).map_or_else(|| name.replace('_', "-"), |(_, v)| v.to_string())
}

/// The DOM event an `on…` attribute listens for, and the payload its
/// handler gets (`onkeydown` a `KeyboardEvent`, `oninput` a `FormEvent`, …).
fn payload(ev: &str) -> Option<(String, &'static str)> {
    let p = match ev {
        "keydown" | "keyup" | "keypress" => "KeyboardData",
        "click" | "contextmenu" | "mousedown" | "mouseup" | "mousemove" | "mouseover" | "mouseout" | "mouseenter"
        | "mouseleave" | "auxclick" | "dblclick" | "doubleclick" => "MouseData",
        "drag" | "dragstart" | "dragend" | "dragenter" | "dragleave" | "dragover" | "drop" | "dragexit" => "DragData",
        "pointerdown" | "pointerup" | "pointermove" | "pointerover" | "pointerout" | "pointerenter"
        | "pointerleave" | "pointercancel" | "gotpointercapture" | "lostpointercapture" | "touchstart" | "touchend"
        | "touchmove" | "touchcancel" => "PointerData",
        "wheel" => "WheelData",
        "resize" => "ResizeData",
        "visible" => "VisibleData",
        "input" | "change" | "submit" | "reset" | "invalid" => "FormData",
        "scroll" | "scrollend" => "ScrollData",
        "mounted" => "",
        "focus" | "blur" | "focusin" | "focusout" | "copy" | "cut" | "paste" | "compositionstart"
        | "compositionupdate" | "compositionend" | "animationstart" | "animationend" | "animationiteration"
        | "transitionend" | "transitionstart" | "transitionrun" | "transitioncancel" | "toggle" | "beforetoggle"
        | "load" | "error" | "abort" | "canplay" | "canplaythrough" | "durationchange" | "emptied" | "encrypted"
        | "ended" | "loadeddata" | "loadedmetadata" | "loadstart" | "pause" | "play" | "playing" | "progress"
        | "ratechange" | "seeked" | "seeking" | "stalled" | "suspend" | "timeupdate" | "volumechange" | "waiting"
        | "select" | "selectstart" | "selectionchange" | "cancel" | "close" => "PlainData",
        _ => return None,
    };
    let dom = if ev == "doubleclick" { "dblclick" } else { ev };
    Some((dom.to_string(), p))
}

fn attr_val(name: &str, value: TokenStream) -> Val {
    let v: Vec<TokenTree> = value.clone().into_iter().collect();
    if let Some(ev) = name.strip_prefix("on") {
        return Val::Event(ev.to_string(), value);
    }
    match v.as_slice() {
        [TokenTree::Literal(l)] => match lit_str(l) {
            Some(s) => match text(&s, l.span()) {
                Node::Fmt(f, r) => Val::Fmt(f, r),
                _ => Val::Static(unbrace(&s)),
            },
            None => Val::Static(l.to_string()),
        },
        [t] if is_ident(t, "true") || is_ident(t, "false") => Val::Present(is_ident(t, "true")),
        _ => Val::Expr(value),
    }
}

/// An expression up to the next top-level `,` (a closure's `|a, b|` kept whole).
fn take_expr(toks: &[TokenTree], i: &mut usize) -> TokenStream {
    let start = *i;
    let mut j = *i;
    if toks.get(j).is_some_and(|t| is_ident(t, "move")) {
        j += 1;
    }
    if is_punct(toks.get(j), '|') && !is_punct(toks.get(j + 1), '|') {
        j += 1;
        while j < toks.len() && !is_punct(toks.get(j), '|') {
            j += 1;
        }
        *i = j + 1;
    }
    while *i < toks.len() && !is_punct(toks.get(*i), ',') {
        *i += 1;
    }
    toks[start..*i].iter().cloned().collect()
}

/// Split on top-level commas (`<…>` counted, `->` not).
fn split_top(toks: &[TokenTree]) -> Vec<Vec<TokenTree>> {
    let (mut out, mut cur, mut depth) = (vec![], vec![], 0i32);
    for (k, t) in toks.iter().enumerate() {
        match t {
            TokenTree::Punct(p) if p.as_char() == '<' => depth += 1,
            TokenTree::Punct(p) if p.as_char() == '>' && !is_punct(toks.get(k.wrapping_sub(1)), '-') => depth -= 1,
            TokenTree::Punct(p) if p.as_char() == ',' && depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(t.clone());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn lit_str(l: &Literal) -> Option<String> {
    let s = l.to_string();
    if let Some(r) = s.strip_prefix('r') {
        let h = r.chars().take_while(|c| *c == '#').count();
        return r.get(h + 1..r.len() - h - 1).map(str::to_string);
    }
    let body = s.strip_prefix('"')?.strip_suffix('"')?;
    let (mut out, mut it) = (String::new(), body.chars().peekable());
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next()? {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '0' => out.push('\0'),
            'x' => {
                let h: String = it.by_ref().take(2).collect();
                out.push(u8::from_str_radix(&h, 16).ok()? as char);
            }
            'u' => {
                let h: String = it.by_ref().skip(1).take_while(|c| *c != '}').collect();
                out.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?)?);
            }
            '\n' => {
                while it.peek().is_some_and(|c| c.is_whitespace()) {
                    it.next();
                }
            }
            c => out.push(c),
        }
    }
    Some(out)
}

fn unbrace(s: &str) -> String {
    s.replace("{{", "{").replace("}}", "}")
}

/// A text: static, or `format!`'s literal and arguments with the names they read.
fn text(s: &str, span: Span) -> Node {
    let (mut fmt, mut args, mut roots, mut it) = (String::new(), vec![], vec![], s.char_indices().peekable());
    while let Some((_, c)) = it.next() {
        match c {
            '{' if it.peek().is_some_and(|(_, c)| *c == '{') => {
                it.next();
                fmt.push_str("{{");
            }
            '}' if it.peek().is_some_and(|(_, c)| *c == '}') => {
                it.next();
                fmt.push_str("}}");
            }
            '}' => fmt.push_str("}}"),
            '{' => {
                let inner: String = it.by_ref().map(|(_, c)| c).take_while(|c| *c != '}').collect();
                let (expr, spec) = match inner
                    .char_indices()
                    .find(|(k, c)| *c == ':' && !inner[k + 1..].starts_with(':') && !inner[..*k].ends_with(':'))
                {
                    Some((k, _)) => (&inner[..k], &inner[k..]),
                    None => (inner.as_str(), ""),
                };
                let ts: TokenStream = expr.parse().unwrap_or_default();
                let ts: TokenStream = ts.into_iter().map(|mut t| (t.set_span(span), t).1).collect();
                captures(&ts, &mut roots);
                args.push(ts);
                fmt.push('{');
                fmt.push_str(spec);
                fmt.push('}');
            }
            c => fmt.push(c),
        }
    }
    if args.is_empty() {
        return Node::Text(unbrace(s));
    }
    let lit = Literal::string(&fmt);
    Node::Fmt(quote!(#lit, #(#args),*), roots)
}

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "false", "fn", "for", "if",
    "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "static", "struct",
    "super", "trait", "true", "type", "unsafe", "use", "where", "while",
];

/// Types no local can be named after.
const PRIMITIVES: &[&str] = &[
    "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
    "f32", "f64",
];

/// The local names `ts` reads (a heuristic over tokens: not a field,
/// method, path, call, macro or type; closure params, `let` and match
/// patterns left out), so a hole's `move` closure can take clones.
fn captures(ts: &TokenStream, out: &mut Vec<String>) {
    let toks: Vec<TokenTree> = ts.clone().into_iter().collect();
    let (mut bound, mut lets) = (vec![], vec![]);
    scan(&toks, out, &mut bound, &mut lets);
    out.retain(|n| !bound.contains(n));
}

fn idents_in(toks: &[TokenTree], into: &mut Vec<String>) {
    for t in toks {
        match t {
            TokenTree::Ident(i) => into.push(i.to_string()),
            TokenTree::Group(g) => idents_in(&g.stream().into_iter().collect::<Vec<_>>(), into),
            _ => {}
        }
    }
}

/// The names pattern `toks` binds: an identifier where a pattern binds
/// (`x`, `ref mut x`, `x @ 1..=3`, `Point { x, .. }`, `Point { x: y }`'s
/// `y`, `(a, b)`), never a path's segment (`crate::Tab::A`), a struct
/// pattern's field name, a tuple struct's or a macro's name, a keyword or
/// a constant (uppercase).
fn pat_bindings(toks: &[TokenTree], into: &mut Vec<String>) {
    for (i, t) in toks.iter().enumerate() {
        match t {
            TokenTree::Group(g) => pat_bindings(&g.stream().into_iter().collect::<Vec<_>>(), into),
            TokenTree::Ident(id) => {
                let (s, next) = (id.to_string(), toks.get(i + 1));
                let binds = s.starts_with(|c: char| c.is_lowercase() || c == '_')
                    && s != "_"
                    && s != "box"
                    && !KEYWORDS.contains(&s.as_str())
                    // A path's segment (`a::`, `::a`) or a field's name (`x: …`).
                    && !is_punct(next, ':')
                    && !(i >= 2 && is_punct(toks.get(i - 1), ':') && is_punct(toks.get(i - 2), ':'))
                    && !is_punct(next, '!')
                    && !matches!(next, Some(TokenTree::Group(_)));
                if binds {
                    into.push(s);
                }
            }
            _ => {}
        }
    }
}

/// `bound`: closure parameters and match patterns (never a capture);
/// `lets`: names a `let` bound, from the end of its statement on (its
/// right side may read an outer name of the same name).
fn scan(toks: &[TokenTree], out: &mut Vec<String>, bound: &mut Vec<String>, lets: &mut Vec<String>) {
    let mut pending: Vec<String> = vec![];
    // Match arms: what comes before each `=>` is a pattern.
    let arrows = toks.windows(2).any(|w| is_punct(Some(&w[0]), '=') && is_punct(Some(&w[1]), '>'));
    let mut i = 0;
    let mut pat_start = 0;
    while i < toks.len() {
        let prev = i.checked_sub(1).and_then(|p| toks.get(p));
        match &toks[i] {
            TokenTree::Punct(p) if arrows && p.as_char() == '=' && is_punct(toks.get(i + 1), '>') => {
                idents_in(&toks[pat_start..i], bound);
                i += 2;
                continue;
            }
            TokenTree::Punct(p) if arrows && p.as_char() == ',' => pat_start = i + 1,
            // `a || b`: no closure.
            TokenTree::Punct(p)
                if p.as_char() == '|'
                    && is_punct(toks.get(i + 1), '|')
                    && !matches!(prev, None | Some(TokenTree::Punct(_)))
                    && !prev.is_some_and(|t| is_ident(t, "move")) =>
            {
                i += 2;
                continue;
            }
            TokenTree::Punct(p)
                if p.as_char() == '|'
                    && (prev.is_none()
                        || matches!(prev, Some(TokenTree::Punct(_)))
                        || prev.is_some_and(|t| is_ident(t, "move"))) =>
            {
                if is_punct(toks.get(i + 1), '|') {
                    i += 2;
                    continue;
                }
                let end = (i + 1..toks.len()).find(|k| is_punct(toks.get(*k), '|')).unwrap_or(toks.len());
                if end == toks.len() {
                    // No closing `|`: not a closure's parameters.
                    i += 1;
                    continue;
                }
                idents_in(&toks[i + 1..end], bound);
                i = end + 1;
                continue;
            }
            TokenTree::Ident(id) if id == "let" => {
                let end = (i + 1..toks.len()).find(|k| is_punct(toks.get(*k), '=')).unwrap_or(toks.len());
                // `if let` / `while let`: the pattern binds for the block that follows.
                let cond = prev.is_some_and(|t| is_ident(t, "if") || is_ident(t, "while")) || is_punct(prev, '&');
                idents_in(&toks[i + 1..end], if cond { &mut *bound } else { &mut pending });
                i = end;
                continue;
            }
            TokenTree::Ident(id) if id == "for" => {
                let end = (i + 1..toks.len()).find(|k| is_ident(&toks[*k], "in")).unwrap_or(toks.len());
                idents_in(&toks[i + 1..end], bound);
                i = end;
                continue;
            }
            TokenTree::Ident(id) => {
                let s = id.to_string();
                let next = toks.get(i + 1);
                let skip = s == "_"
                    || KEYWORDS.contains(&s.as_str())
                    || PRIMITIVES.contains(&s.as_str())
                    || prev.is_some_and(|t| is_ident(t, "as"))
                    || s.starts_with(|c: char| c.is_uppercase())
                    || is_punct(prev, '.')
                    || (is_punct(prev, ':') && i >= 2 && is_punct(toks.get(i - 2), ':'))
                    || (is_punct(next, ':') && is_punct(toks.get(i + 2), ':'))
                    || is_colon(toks, i + 1)
                    // A call: a function (a local closure called in a hole is
                    // cloned in by a `let` of the hole's own, or is a `Callback`).
                    || matches!(next, Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis);
                if is_punct(next, '!')
                    && !KEYWORDS.contains(&s.as_str())
                    && matches!(toks.get(i + 2), Some(TokenTree::Group(_)))
                {
                    // A macro: the `{name}`s of its format string, then its arguments.
                    // `field!(name)` (aio's form field) names a struct field: no capture.
                    if s == "field" {
                        i += 3;
                        continue;
                    }
                    if let Some(TokenTree::Group(g)) = toks.get(i + 2) {
                        let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                        // A view inside: the names its nodes read, not its tags.
                        if s == "rsx" {
                            if let Ok(nodes) = parse_nodes(&inner) {
                                let mut r = vec![];
                                node_captures(&nodes, &mut r);
                                out.extend(r.into_iter().filter(|n| !bound.contains(n) && !lets.contains(n)));
                            }
                            i += 3;
                            continue;
                        }
                        if let Some(TokenTree::Literal(l)) = inner.first()
                            && let Some(s) = lit_str(l)
                            && let Node::Fmt(_, r) = text(&s, l.span())
                        {
                            out.extend(r);
                        }
                        scan(&inner, out, bound, lets);
                    }
                    i += 3;
                    continue;
                }
                if !skip && !out.contains(&s) && !lets.contains(&s) {
                    out.push(s);
                }
            }
            TokenTree::Group(g) => scan(&g.stream().into_iter().collect::<Vec<_>>(), out, bound, lets),
            TokenTree::Punct(p) if p.as_char() == ';' => lets.append(&mut pending),
            _ => {}
        }
        i += 1;
    }
}

/// `{ let a = a.clone(); … }` for the names a hole reads.
fn clones(names: &[String]) -> TokenStream {
    let n = names.iter().map(|n| match n.strip_prefix("r#") {
        Some(raw) => Ident::new_raw(raw, Span::call_site()),
        None => Ident::new(n, Span::call_site()),
    });
    quote!(#(#[allow(unused_mut)] let mut #n = ::std::clone::Clone::clone(&#n);)*)
}

fn node_captures(nodes: &[Node], out: &mut Vec<String>) {
    for n in nodes {
        match n {
            Node::Text(_) => {}
            Node::Fmt(_, r) => out.extend(r.iter().cloned()),
            Node::Expr(e) => captures(e, out),
            Node::El { attrs, spreads, children, key, .. } => {
                for (_, v) in attrs {
                    match v {
                        Val::Fmt(_, r) => out.extend(r.iter().cloned()),
                        Val::Event(_, e) | Val::Expr(e) => captures(e, out),
                        _ => {}
                    }
                }
                spreads.iter().for_each(|e| captures(e, out));
                key.iter().for_each(|k| captures(k, out));
                node_captures(children, out);
            }
            Node::Comp { props, spreads, children, key, .. } => {
                for (_, e) in props {
                    match e.clone().into_iter().collect::<Vec<_>>().as_slice() {
                        [TokenTree::Literal(l)] => {
                            if let Some(Node::Fmt(_, r)) = lit_str(l).map(|s| text(&s, l.span())) {
                                out.extend(r)
                            }
                        }
                        _ => captures(e, out),
                    }
                }
                spreads.iter().for_each(|e| captures(e, out));
                key.iter().for_each(|k| captures(k, out));
                node_captures(children, out);
            }
            Node::For { pat, expr, body } => {
                captures(expr, out);
                let mut inner = vec![];
                node_captures(body, &mut inner);
                let mut b = vec![];
                idents_in(&pat.clone().into_iter().collect::<Vec<_>>(), &mut b);
                out.extend(inner.into_iter().filter(|n| !b.contains(n)));
            }
            Node::If { cond, then, otherwise } => {
                captures(cond, out);
                let (mut inner, bound) = (vec![], let_bound(cond));
                node_captures(then, &mut inner);
                out.extend(inner.into_iter().filter(|n| !bound.contains(n)));
                node_captures(otherwise, out);
            }
            Node::Match { expr, arms } => {
                captures(expr, out);
                for (pat, guard, nodes) in arms {
                    let (mut inner, mut bound) = (vec![], vec![]);
                    pat_bindings(&pat.clone().into_iter().collect::<Vec<_>>(), &mut bound);
                    guard.iter().for_each(|g| captures(g, &mut inner));
                    node_captures(nodes, &mut inner);
                    out.extend(inner.into_iter().filter(|n| !bound.contains(n)));
                }
            }
        }
    }
    let mut seen = vec![];
    out.retain(|n| {
        let fresh = !seen.contains(n);
        seen.push(n.clone());
        fresh
    });
}

/// An arm's key for `branch`: its index, or `u32::MAX` (made again on every
/// change) when its pattern binds a name. `bound`: what it binds ([`pat_bindings`]).
fn arm_key(k: u32, bound: &[String]) -> TokenStream {
    let k = if bound.is_empty() { k } else { u32::MAX };
    quote!(#k)
}

/// The names an `if`'s `let`s bind (`if let Some(x) = a && let Ok(y) = b`: `x`, `y`).
fn let_bound(cond: &TokenStream) -> Vec<String> {
    let toks: Vec<TokenTree> = cond.clone().into_iter().collect();
    let mut out = vec![];
    for (i, _) in toks.iter().enumerate().filter(|(_, t)| is_ident(t, "let")) {
        let end = (i + 1..toks.len()).find(|k| is_punct(toks.get(*k), '=')).unwrap_or(toks.len());
        pat_bindings(&toks[i + 1..end], &mut out);
    }
    out
}

/// Is `n` a placeholder in its template (an anchor its content goes before)?
fn anchored(n: &Node) -> bool {
    matches!(n, Node::Expr(_) | Node::Comp { .. } | Node::For { .. } | Node::If { .. } | Node::Match { .. })
}

struct Tpl {
    bytes: Vec<u8>,
    count: u32,
    marks: u32,
    holes: Vec<TokenStream>,
}

impl Tpl {
    fn str(&mut self, s: &str) {
        self.bytes.extend_from_slice(&(s.len() as u32).to_le_bytes());
        self.bytes.extend_from_slice(s.as_bytes());
    }

    fn mark(&mut self) -> TokenStream {
        let b = Ident::new("__air_b", Span::mixed_site());
        let k = self.marks;
        self.marks += 1;
        quote!(#b + #k)
    }

    /// `n` in pre-order; `root`: always marked (the view's range).
    fn node(&mut self, n: &Node, root: bool) {
        self.count += 1;
        match n {
            Node::Text(s) => {
                self.bytes.push(2);
                self.str(s);
                if root {
                    self.mark();
                }
            }
            Node::Fmt(f, r) => {
                self.bytes.push(3);
                let id = self.mark();
                let c = clones(r);
                self.holes
                    .push(quote!(air::__rt::text(#id, { #c ::std::boxed::Box::new(move || ::std::format!(#f)) });));
            }
            Node::El { tag, attrs, spreads, children, .. } => {
                let outer = IN_SVG.get();
                // 5: an element in SVG's namespace though its template has no `svg` above it.
                let svg_root = root && tag != "svg" && (outer || SVG_ONLY.contains(&tag.as_str()));
                self.bytes.push(if svg_root { 5 } else { 1 });
                self.str(tag);
                let stat: Vec<(&String, String)> = attrs
                    .iter()
                    .filter_map(|(k, v)| match v {
                        Val::Static(_) if k == "innerHTML" => None,
                        Val::Static(s) => Some((k, s.clone())),
                        Val::Present(b) if text_bool(k) => Some((k, b.to_string())),
                        Val::Present(true) => Some((k, String::new())),
                        _ => None,
                    })
                    .collect();
                self.bytes.push(stat.len() as u8);
                for (k, v) in &stat {
                    self.str(k);
                    self.str(v);
                }
                self.bytes.extend_from_slice(&(children.len() as u16).to_le_bytes());
                // `innerHTML` is a property set at mount, as the page's guard sees it, not
                // a template attribute.
                let dynamic = !spreads.is_empty()
                    || attrs.iter().any(|(k, v)| k == "innerHTML" || !matches!(v, Val::Static(_) | Val::Present(_)));
                self.bytes.push(dynamic as u8);
                if dynamic || root {
                    let id = self.mark();
                    for (k, v) in attrs {
                        let hole = match v {
                            Val::Static(s) if k == "innerHTML" => {
                                let (head, set) = attr_fn();
                                quote!(air::__rt::attr(#id, #k, ::std::boxed::Box::new(#head #set(air::IntoAttr::into_attr(#s))));)
                            }
                            Val::Fmt(f, r) => {
                                let c = clones(r);
                                let (head, set) = attr_fn();
                                quote!(air::__rt::attr(#id, #k, { #c ::std::boxed::Box::new(#head #set(air::IntoAttr::into_attr(::std::format!(#f)))) });)
                            }
                            Val::Expr(e) => {
                                let (head, set) = attr_fn();
                                hole_fn(
                                    e,
                                    head,
                                    |v| quote!(#set(air::IntoAttr::into_attr(#v))),
                                    |e| quote!(air::__rt::attr(#id, #k, #e);),
                                )
                            }
                            Val::Event(ev, e) => {
                                let mut r = vec![];
                                captures(e, &mut r);
                                let c = clones(&r);
                                let (dom, data) = payload(ev).expect("checked at parse");
                                if dom == "mounted" {
                                    quote!(air::__rt::mounted(#id, { #c air::__rt::mounted_fn(#e) });)
                                } else {
                                    let data = Ident::new(data, Span::call_site());
                                    // A closure gets its parameter's type from a direct bound.
                                    let closure = is_closure(e);
                                    let h = if closure {
                                        quote!(air::__rt::handler_fn::<air::__rt::#data, _, _>(::std::boxed::Box::new(#e)))
                                    } else {
                                        quote!(air::__rt::handler::<air::__rt::#data, _>(#e))
                                    };
                                    quote!(air::__rt::on(#id, #dom, { #c #h });)
                                }
                            }
                            _ => continue,
                        };
                        self.holes.push(hole);
                    }
                    // After the element's own: what is handed in wins.
                    for e in spreads {
                        let mut r = vec![];
                        captures(e, &mut r);
                        let c = clones(&r);
                        self.holes.push(quote!(air::__rt::spread(#id, { #c #e });));
                    }
                }
                let svg = (outer || svg_root || tag == "svg") && tag != "foreignObject";
                IN_SVG.set(svg);
                for c in children {
                    self.node(c, false);
                }
                IN_SVG.set(outer);
            }
            n => {
                self.bytes.push(4);
                let id = self.mark();
                let hole = match n {
                    Node::Expr(e) => hole_fn(
                        e,
                        quote!(move ||),
                        |v| quote!(air::IntoChild::into_child(#v)),
                        |e| quote!(air::__rt::child(#id, #e);),
                    ),
                    Node::Comp { path, key, .. } => {
                        let mut r = vec![];
                        node_captures(std::slice::from_ref(n), &mut r);
                        let c = clones(&r);
                        let call = comp(n);
                        // Its name and key, for the tree of mounted components.
                        let name = path.iter().rev().find_map(|t| match t {
                            TokenTree::Ident(i) => Some(i.to_string()),
                            _ => None,
                        });
                        let key = match key {
                            Some(k) => quote!({
                                #[allow(unused_imports)]
                                use air::__rt::{KeyByDebug as _, KeyByDisplay as _, KeyByNone as _};
                                (&&air::__rt::KeyOf(&(#k))).key_text()
                            }),
                            None => quote!(::core::option::Option::None),
                        };
                        quote!(air::__rt::comp(#id, #name, #key, { #c ::std::boxed::Box::new(move || #call) });)
                    }
                    Node::For { pat, expr, body: rows } => {
                        let key = match rows.first() {
                            Some(Node::El { key: Some(k), .. } | Node::Comp { key: Some(k), .. }) => k.clone(),
                            _ => {
                                return self.holes.push(
                                    quote!(::core::compile_error!("air: a `for` in rsx! needs `key:` on its row");),
                                );
                            }
                        };
                        let (mut ce, mut ck, mut cr) = (vec![], vec![], vec![]);
                        captures(expr, &mut ce);
                        let mut b = vec![];
                        idents_in(&pat.clone().into_iter().collect::<Vec<_>>(), &mut b);
                        captures(&key, &mut ck);
                        ck.retain(|n| !b.contains(n));
                        node_captures(rows, &mut cr);
                        cr.retain(|n| !b.contains(n));
                        let (ce, ck, cr) = (clones(&ce), clones(&ck), clones(&cr));
                        let view = body(rows);
                        quote!(#[allow(unused_variables)] air::__rt::each(#id,
                            { #ce move || { #ce ::std::iter::IntoIterator::into_iter(#expr).collect::<::std::vec::Vec<_>>() } },
                            { #ck move |#pat: &_| (#key).to_owned() },
                            { #cr move |#pat| #view });)
                    }
                    // `if let` and `match`: a branch hole, its view made again when
                    // another arm matches or what the arm binds changes.
                    Node::If { cond, then, otherwise } if cond.clone().into_iter().any(|t| is_ident(&t, "let")) => {
                        let mut r = vec![];
                        node_captures(std::slice::from_ref(n), &mut r);
                        let c = clones(&r);
                        let (t, o) = (body(then), body(otherwise));
                        let k = arm_key(0, &let_bound(cond));
                        quote!(air::__rt::branch(#id, { #c ::std::boxed::Box::new(move || { #c if #cond { (#k, #t) } else { (1u32, #o) } }) });)
                    }
                    Node::Match { expr, arms } => {
                        let mut r = vec![];
                        node_captures(std::slice::from_ref(n), &mut r);
                        let c = clones(&r);
                        let arms = arms.iter().enumerate().map(|(k, (pat, guard, nodes))| {
                            let mut bound = vec![];
                            pat_bindings(&pat.clone().into_iter().collect::<Vec<_>>(), &mut bound);
                            let k = arm_key(k as u32, &bound);
                            let guard = guard.as_ref().map(|g| quote!(if #g));
                            let view = body(nodes);
                            quote!(#pat #guard => (#k, #view),)
                        });
                        quote!(air::__rt::branch(#id, { #c ::std::boxed::Box::new(move || { #c match #expr { #(#arms)* } }) });)
                    }
                    Node::If { cond, then, otherwise } => {
                        let (mut a, mut b, mut c) = (vec![], vec![], vec![]);
                        captures(cond, &mut a);
                        node_captures(then, &mut b);
                        node_captures(otherwise, &mut c);
                        let (a, b, c) = (clones(&a), clones(&b), clones(&c));
                        let (t, o) = (body(then), body(otherwise));
                        quote!(air::__rt::show(#id, { #a ::std::boxed::Box::new(move || #cond) }, { #b ::std::boxed::Box::new(move || #t) }, { #c ::std::boxed::Box::new(move || #o) });)
                    }
                    _ => unreachable!(),
                };
                self.holes.push(hole);
            }
        }
    }
}

/// An attribute hole's closure head and what hands it its value: the
/// value goes to the hole's slot, so it may borrow what the closure read.
fn attr_fn() -> (TokenStream, TokenStream) {
    let s = Ident::new("__air_s", Span::mixed_site());
    (quote!(move |#s: &mut air::__rt::AttrSlot|), quote!(#s.set))
}

/// A hole's closure (`head`, then `into` of the value): the names it reads
/// are cloned in, and again per run (as a component's props), so `{match
/// who { .. }}` or `{note.map(..)}` may take them by value; a single name's
/// type decides whether it follows anything (a signal does).
fn hole_fn(
    e: &TokenStream,
    head: TokenStream,
    into: impl Fn(TokenStream) -> TokenStream,
    call: impl Fn(TokenStream) -> TokenStream,
) -> TokenStream {
    let v: Vec<TokenTree> = e.clone().into_iter().collect();
    // A constant or a static (`CLOUD`, `ACCENT`): read where it is, no local to clone.
    if let [TokenTree::Ident(i)] = v.as_slice()
        && i.to_string().starts_with(|c: char| c.is_uppercase())
    {
        let v = into(quote!(::std::clone::Clone::clone(&#i)));
        return call(quote!(::std::boxed::Box::new(#head #v)));
    }
    if let [TokenTree::Ident(i)] = v.as_slice() {
        let v = into(quote!(::std::clone::Clone::clone(&#i)));
        return call(quote!({ let #i = ::std::clone::Clone::clone(&#i); ::std::boxed::Box::new(#head #v) }));
    }
    let mut r = vec![];
    captures(e, &mut r);
    let c = clones(&r);
    // The per-run clones outlive `into`: `{t(&x)}` may borrow them. A lone
    // expression is `into`'s argument, so what it makes lives as long
    // (`{t(&sig.get())}`); a block's tail drops its temporaries at its end.
    let v = into(if v.iter().any(|t| is_punct(Some(t), ';')) { quote!({ #e }) } else { e.clone() });
    call(quote!({ #c ::std::boxed::Box::new(#head { #c #v }) }))
}

/// `Row { a: x, "data-t": t, ..more, children… }` →
/// `Row(builder().a(move || x).__attr("data-t", move || t).__spread(more).children(move || …).build())`:
/// every value a closure (each clones the names it reads), so a `Prop`
/// follows it and any other prop takes its value once; a closure given
/// as a value (a handler) goes to `__fn_a`, which types its parameters.
fn comp(n: &Node) -> TokenStream {
    let Node::Comp { path, props, spreads, children, .. } = n else { unreachable!() };
    let path: TokenStream = path.iter().cloned().collect();
    // Cloned in, and again per call: a `Prop` calls it again, so it never
    // moves what it captured.
    let thunk = |e: &TokenStream| {
        let mut r = vec![];
        captures(e, &mut r);
        let c = clones(&r);
        quote!({ #c move || { #c #e } })
    };
    let mut calls = vec![];
    for (k, v) in props {
        // `"row-{key}"`: formatted, as in an attribute.
        let formatted = match v.clone().into_iter().collect::<Vec<_>>().as_slice() {
            [TokenTree::Literal(l)] => match lit_str(l).map(|s| text(&s, l.span())) {
                Some(Node::Fmt(f, _)) => Some(quote!(::std::format!(#f))),
                _ => None,
            },
            _ => None,
        };
        let v = formatted.as_ref().unwrap_or(v);
        let closure = is_closure(v);
        calls.push(match k {
            // A constant needs no closure (and a `Prop` of it holds no state).
            // A literal string converts (`"a"` to a `String`); any other
            // constant is the prop's own type (`5` a `u32`).
            PropName::Field(f) if constant(v) => {
                let text = matches!(v.clone().into_iter().next(), Some(TokenTree::Literal(l)) if lit_str(&l).is_some());
                let f =
                    format_ident!("__{}_{}", if text { "fn" } else { "val" }, f.to_string().trim_start_matches("r#"));
                quote!(.#f(#v))
            }
            PropName::Attr(a) if constant(v) => {
                let a = attr_name(a);
                quote!(.__attr_const(#a, #v))
            }
            PropName::Field(f) if closure => {
                let f = format_ident!("__fn_{}", f.to_string().trim_start_matches("r#"));
                let mut r = vec![];
                captures(v, &mut r);
                let c = clones(&r);
                quote!(.#f({ #c #v }))
            }
            PropName::Field(f) => {
                let t = thunk(v);
                quote!(.#f(#t))
            }
            PropName::Attr(a) => {
                let a = attr_name(a);
                let t = thunk(v);
                quote!(.__attr(#a, #t))
            }
        });
    }
    for e in spreads {
        let mut r = vec![];
        captures(e, &mut r);
        let c = clones(&r);
        calls.push(quote!(.__spread({ #c #e })));
    }
    if !children.is_empty() {
        let v = body(children);
        let mut r = vec![];
        node_captures(children, &mut r);
        let c = clones(&r);
        calls.push(quote!(.children({ #c move || #v })));
    }
    let builder = match props.iter().any(|(k, _)| matches!(k, PropName::Attr(_))) || !spreads.is_empty() {
        // Given attributes: a component that takes none says so.
        true => quote!(air::__rt::with_attributes(air::__rt::builder_of(__air_c))),
        false => quote!(air::__rt::builder_of(__air_c)),
    };
    quote!({
        #[allow(unused_imports)]
        use air::__rt::{GlobalAttributes as _, GlobalAttributeNames as _};
        let __air_c = &#path;
        air::Component::call(__air_c, #builder #(#calls)* .build())
    })
}

/// Is `v` a closure (`move |e| …`), or a block ending in one
/// (`{ let id = id.clone(); move |_| … }`)? Its parameters are typed then.
fn is_closure(v: &TokenStream) -> bool {
    let t: Vec<TokenTree> = v.clone().into_iter().collect();
    if let [TokenTree::Group(g)] = t.as_slice()
        && g.delimiter() == Delimiter::Brace
    {
        let inner: Vec<TokenTree> = g.stream().into_iter().collect();
        let tail = inner.iter().rposition(|t| is_punct(Some(t), ';')).map_or(0, |k| k + 1);
        return is_closure(&inner[tail..].iter().cloned().collect());
    }
    let at = usize::from(t.first().is_some_and(|t| is_ident(t, "move")));
    is_punct(t.get(at), '|')
}

/// air's `is_text_bool`: `true`/`false` is text for `aria-*`, `data-*` and
/// HTML's enumerated attributes, presence for the rest.
fn text_bool(name: &str) -> bool {
    name.starts_with("aria-")
        || name.starts_with("data-")
        || matches!(name, "draggable" | "contenteditable" | "spellcheck")
}

/// Is `v` a constant: a literal (`"a"`, `3`, `-1.5`, `true`) or an enum's
/// variant (`Variant::Primary`)?
fn constant(v: &TokenStream) -> bool {
    let t: Vec<TokenTree> = v.clone().into_iter().collect();
    match t.as_slice() {
        [TokenTree::Literal(_)] => true,
        [TokenTree::Punct(p), TokenTree::Literal(_)] => p.as_char() == '-',
        [t] => is_ident(t, "true") || is_ident(t, "false"),
        [.., TokenTree::Ident(last)] if t.len() >= 4 => {
            last.to_string().starts_with(|c: char| c.is_uppercase())
                && t.iter().enumerate().all(|(k, x)| match x {
                    TokenTree::Ident(_) => true,
                    TokenTree::Punct(p) => p.as_char() == ':' && (k % 3 != 0),
                    _ => false,
                })
        }
        _ => false,
    }
}

/// The view of `nodes`: one template and its holes, built where it is
/// placed (the names it reads are cloned in first).
fn body(nodes: &[Node]) -> TokenStream {
    if nodes.is_empty() {
        return quote!(::core::result::Result::<air::Tree, air::ViewError>::Ok(air::Tree::empty()));
    }
    // Its index in the call: pre-order, as `templates` lists them.
    let idx = HOT.with_borrow_mut(|h| {
        h.next += 1;
        if let Some(m) = &mut h.made {
            m.push(Made::default());
        }
        h.next - 1
    });
    let mut t = Tpl { bytes: vec![], count: 0, marks: 0, holes: vec![] };
    // A view is a range: what goes before a first anchor needs a start of its own.
    let lead = anchored(&nodes[0]);
    let mut roots = nodes.len() as u32;
    if lead {
        t.count += 1;
        t.bytes.push(4);
        t.mark();
        roots += 1;
    }
    let mut first_last = (0, 0);
    for (k, n) in nodes.iter().enumerate() {
        let m = t.marks;
        t.node(n, true);
        if k == 0 && !lead {
            first_last.0 = m;
        }
        first_last.1 = m;
    }
    let (first, last) = first_last;
    let Tpl { bytes, count, marks, holes } = t;
    let (bytes, count, at) = HOT.with_borrow_mut(|h| match &mut h.made {
        // Read for a hot reload: the markup left out of the shape.
        Some(m) => {
            m[idx as usize] = Made { nodes: bytes, count };
            (quote!(__markup), quote!(__count), quote!(__at))
        }
        None => {
            let (file, line, col) = h.at.clone().unwrap_or_default();
            let at = quote!(air::__rt::At { file: if air::__rt::DEV { #file } else { "" }, line: #line, col: #col, idx: #idx });
            (Literal::byte_string(&bytes).into_token_stream(), count.into_token_stream(), at)
        }
    });
    let (tn, b) = (Ident::new("__AIR_T", Span::mixed_site()), Ident::new("__air_b", Span::mixed_site()));
    let mut names = vec![];
    node_captures(nodes, &mut names);
    let c = clones(&names);
    quote!({
        #c
        ::core::result::Result::<air::Tree, air::ViewError>::Ok(air::Tree::new(move || {
            static #tn: air::__rt::Template = air::__rt::Template { nodes: #bytes, count: #count, roots: #roots, marks: #marks, first: #first, last: #last, at: #at };
            let #b = air::__rt::tpl(&#tn);
            #(#holes)*
            air::__rt::view(&#tn, #b)
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(src: &str) -> Vec<String> {
        let mut out = vec![];
        captures(&src.parse().unwrap(), &mut out);
        out
    }

    #[test]
    fn a_hole_clones_the_names_it_reads() {
        assert_eq!(names("last || (c.href.is_none() && c.onclick.is_none())"), ["last", "c"], "`||` is no closure");
        assert_eq!(names("move |e| { if !keys.fire(e.clone()) { tell.call(W::KeyDown(e)) } }"), ["keys", "tell"]);
        assert_eq!(names("|x| x + y"), ["y"]);
        assert_eq!(names("|| a"), ["a"]);
        assert_eq!(names("move |_| start(x)"), ["x"], "a call is a function's");
        assert_eq!(names("{ let start = start.clone(); move |_| start() }"), ["start"], "a closure, cloned in");
        assert_eq!(names("{ let (h, k) = (h.clone(), k.clone()); move |e| f(h, k, e, z) }"), ["h", "k", "z"]);
        assert_eq!(names("{ let n = 1; n + m }"), ["m"], "a let's own name, after it");
        assert_eq!(names("move |_| if let Some(f) = on { f.call(v) }"), ["on", "v"]);
        assert_eq!(names("for x in xs { g(x) }"), ["xs"]);
        assert_eq!(names("if a && let Some(t) = top() { t.go() }"), ["a"], "a let chain");
        assert_eq!(names("form.bind(aio::field!(name))"), ["form"], "field!(name) names a field");
        assert_eq!(
            names("async { let n = get().await; rsx! { p { \"{n} {m}\" } } }"),
            ["m"],
            "a let read by a view inside"
        );
    }

    #[test]
    fn a_pattern_binds_only_where_a_name_binds() {
        let bound = |src: &str| {
            let mut out = vec![];
            pat_bindings(&src.parse::<TokenStream>().unwrap().into_iter().collect::<Vec<_>>(), &mut out);
            out
        };
        assert_eq!(bound("Some(x)"), ["x"]);
        assert_eq!(bound("n @ 1..=3"), ["n"]);
        assert_eq!(bound("Point { x, y: ref mut z, .. }"), ["x", "z"], "a field's name binds only as shorthand");
        assert_eq!(bound("(a, _, box b)"), ["a", "b"]);
        let none: [&str; 0] = [];
        assert_eq!(bound("crate::Tab::A"), none, "a path's segments");
        assert_eq!(bound("self::tabs::Tab::B(_) | super::Tab::C { .. }"), none);
        assert_eq!(bound("(Mode::Edit, true) | None | MAX"), none, "variants, keywords, constants");
    }

    #[test]
    fn a_block_ending_in_a_closure_is_a_closure() {
        assert!(is_closure(&"move |e| f(e)".parse().unwrap()));
        assert!(is_closure(&"{ let r = row.clone(); move |e| f(r, e) }".parse().unwrap()));
        assert!(!is_closure(&"{ handler }".parse().unwrap()));
        assert!(!is_closure(&"a || b".parse().unwrap()));
    }
}

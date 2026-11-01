//! `#[cell]` makes a struct a cell; `#[methods]` makes its `pub fn (&mut self, …)`
//! callable by name (so the log replays) and typed for callers (`<Name>Calls`).
use proc_macro::TokenStream;
use proc_macro2::{Ident, TokenStream as Tokens, TokenTree};
use quote::{format_ident, quote};
use syn::{
    Attribute, Expr, FnArg, GenericArgument, ImplItem, ImplItemFn, ItemImpl, ItemStruct, LitInt, LitStr, Pat, Path,
    PathArguments, ReturnType, Stmt, Type, Visibility, parse_macro_input, parse_quote, spanned::Spanned,
};

#[derive(Default)]
struct Flags {
    persist: bool,
    shared: bool,
    version: Option<u32>,
    id: Option<String>,
    migrate: Option<Path>,
    visible: Option<Path>,
    save: Option<Path>,
    restore: Option<Path>,
    retire: Vec<LitStr>,
    custom_default: bool,
    quiet: bool,
    coalesce: bool,
    machine: Option<Ident>,
}

/// `#[cell(persist, shared, version = 2, id = "todo", migrate = up)]`;
/// `custom_default` when the struct implements `Default` itself;
/// `diagnostics = false` keeps its calls out of the timeline and reports;
/// `coalesce` makes every plain method `#[coalesce]` (aio's debounced writes);
/// `machine = field` guards methods by an `aio::machine!` field;
/// `visible = f` (`fn(&Self, &By) -> Self`, or `fn(&Self, &Caller) -> Self`
/// for who with their role) filters what a viewer gets, and
/// `#[server]` on a field keeps it off the wire. What is saved: `#[transient]`
/// on a field keeps it off disk, `save = f` (`fn(&Self) -> Self`) shapes it,
/// `restore = f` (`fn(Self) -> Self`) repairs it on every boot. In an aio file,
/// `#[table("name")]` on a field (`#[table("name", key = "col")]` on a map)
/// keeps it as the rows of aio's `db:` table `name`, and `#[aio_cell]` on a field
/// whose type is a cell keeps it as that aio cell's slice. `retire = ["old"]`
/// deletes cells no build reads any more from the file at its boot.
#[proc_macro_attribute]
pub fn cell(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut f = Flags::default();
    let parser = syn::meta::parser(|m| {
        if m.path.is_ident("persist") {
            f.persist = true;
        } else if m.path.is_ident("shared") {
            f.shared = true;
        } else if m.path.is_ident("version") {
            f.version = Some(m.value()?.parse::<LitInt>()?.base10_parse()?);
        } else if m.path.is_ident("id") {
            f.id = Some(m.value()?.parse::<LitStr>()?.value());
        } else if m.path.is_ident("custom_default") {
            f.custom_default = true;
        } else if m.path.is_ident("coalesce") {
            f.coalesce = true;
        } else if m.path.is_ident("machine") {
            f.machine = Some(m.value()?.parse()?);
        } else if m.path.is_ident("diagnostics") {
            f.quiet = !m.value()?.parse::<syn::LitBool>()?.value;
        } else if m.path.is_ident("visible") {
            f.visible = Some(m.value()?.parse()?);
        } else if m.path.is_ident("migrate") {
            f.migrate = Some(m.value()?.parse()?);
        } else if m.path.is_ident("save") {
            f.save = Some(m.value()?.parse()?);
        } else if m.path.is_ident("restore") {
            f.restore = Some(m.value()?.parse()?);
        } else if m.path.is_ident("retire") {
            let list: syn::ExprArray = m.value()?.parse()?;
            for e in list.elems {
                match e {
                    Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => f.retire.push(s),
                    e => return Err(syn::Error::new(e.span(), "retire = [\"cell\", …]: the retired cells' ids")),
                }
            }
        } else {
            return Err(m.error(
                "unknown cell flag: expected persist, shared, version, id, migrate, visible, save, restore, retire, machine, diagnostics, coalesce or custom_default",
            ));
        }
        Ok(())
    });
    parse_macro_input!(attr with parser);
    let mut s = parse_macro_input!(item as ItemStruct);
    // `#[server]` fields: kept and saved, never sent to clients.
    // `#[transient]` fields: kept in memory, never saved.
    let (mut hidden, mut transient) = (Vec::new(), Vec::new());
    for field in s.fields.iter_mut() {
        for (flag, into) in [("server", &mut hidden), ("transient", &mut transient)] {
            if take(&mut field.attrs, flag).is_none() {
                continue;
            }
            if field.attrs.iter().any(|a| a.path().is_ident("serde")) {
                let msg = format!("a #[{flag}] field can't have #[serde(...)] yet");
                return syn::Error::new(field.span(), msg).to_compile_error().into();
            }
            match &field.ident {
                Some(i) => into.push(i.to_string()),
                None => {
                    let msg = format!("#[{flag}] needs a named field");
                    return syn::Error::new(field.span(), msg).to_compile_error().into();
                }
            }
        }
    }
    // A shared cell sends every field but its #[server] ones to each client:
    // an unambiguous credential among them is refused, never sent.
    if f.shared && f.visible.is_none() {
        let sent = s.fields.iter().filter_map(|x| x.ident.as_ref()).filter(|i| !hidden.contains(&i.to_string()));
        if let Some(i) = sent.into_iter().find(|i| credential(&i.to_string())) {
            let msg = format!(
                "`{i}` looks like a credential, and this cell is shared: every client would get it. \
                 Mark it #[server] (kept and saved, never sent), or rename it if it isn't one"
            );
            return syn::Error::new(i.span(), msg).to_compile_error().into();
        }
    }
    // `#[table("name")]` / `#[table("name", key = "col")]`: aio's `db:` table.
    let mut tables = Vec::new();
    for field in s.fields.iter_mut() {
        let Some(a) = take(&mut field.attrs, "table") else { continue };
        let (mut name, mut key, mut cols) = (None::<LitStr>, None::<LitStr>, None::<LitStr>);
        let parsed = a.parse_args_with(|p: syn::parse::ParseStream| {
            name = Some(p.parse()?);
            while p.parse::<Option<syn::Token![,]>>()?.is_some() {
                let k: syn::Ident = p.parse()?;
                let into = match k.to_string().as_str() {
                    "key" if key.is_none() => &mut key,
                    "columns" if cols.is_none() => &mut cols,
                    _ => return Err(syn::Error::new(k.span(), "expected key = \"col\" or columns = \"…\"")),
                };
                p.parse::<syn::Token![=]>()?;
                *into = Some(p.parse()?);
            }
            Ok(())
        });
        let (Ok(()), Some(name), Some(i)) = (parsed, name, &field.ident) else {
            let msg = "#[table(\"name\")] on a named field, or #[table(\"name\", key = \"col\")] for a map; \
                       columns = \"id INTEGER PRIMARY KEY, …\" declares its schema";
            return syn::Error::new(field.span(), msg).to_compile_error().into();
        };
        if field.attrs.iter().any(|a| a.path().is_ident("serde")) {
            return syn::Error::new(field.span(), "a #[table] field can't have #[serde(...)] yet")
                .to_compile_error()
                .into();
        }
        let (i, name) = (i.to_string(), name.value());
        let key = match key {
            Some(k) => quote!(::std::option::Option::Some(#k)),
            None => quote!(::std::option::Option::None),
        };
        let cols = match cols {
            Some(c) => quote!(::std::option::Option::Some(#c)),
            None => quote!(::std::option::Option::None),
        };
        tables.push(quote!((#i, #name, #key, #cols)));
    }
    // `#[aio_cell]`: the field is the aio cell its type is, its whole slice.
    let mut slices = Vec::new();
    for field in s.fields.iter_mut() {
        let Some(a) = take(&mut field.attrs, "aio_cell") else { continue };
        let (Some(i), syn::Meta::Path(_)) = (&field.ident, &a.meta) else {
            let msg = "#[aio_cell] on a named field, whose type is the aio cell (`#[cell(persist, id = \"…\")]`)";
            return syn::Error::new(field.span(), msg).to_compile_error().into();
        };
        let name = i.to_string();
        if transient.contains(&name) || field.attrs.iter().any(|a| a.path().is_ident("serde")) {
            let msg = "a #[aio_cell] field can't be #[transient] or have #[serde(...)]";
            return syn::Error::new(field.span(), msg).to_compile_error().into();
        }
        let ty = &field.ty;
        slices.push(quote!(::aio::compat::data::Slice::of::<#ty>(#name)));
    }
    if !slices.is_empty() && (!f.persist || !tables.is_empty()) {
        let msg = "#[aio_cell] fields need #[cell(persist)], and their #[table]s go on the field's cell type";
        return syn::Error::new(s.ident.span(), msg).to_compile_error().into();
    }
    if !f.persist && !tables.is_empty() {
        return syn::Error::new(s.ident.span(), "#[table] needs #[cell(persist)]").to_compile_error().into();
    }
    if !f.persist && (!transient.is_empty() || f.save.is_some() || f.restore.is_some() || !f.retire.is_empty()) {
        let msg = "#[transient], save, restore and retire need #[cell(persist)]";
        return syn::Error::new(s.ident.span(), msg).to_compile_error().into();
    }
    let name = &s.ident;
    let id = f.id.unwrap_or_else(|| snake(&name.to_string()));
    let version = f.version.unwrap_or(1);
    let (persist, shared) = (f.persist, f.shared);
    let migrate = f.migrate.map(|p| {
        quote! {
            fn migrate(old: ::aio::serde_json::Value, from: u32) -> ::std::result::Result<Self, ::std::string::String> {
                #p(old, from)
            }
        }
    });
    let transient =
        (!transient.is_empty()).then(|| quote!(const TRANSIENT: &'static [&'static str] = &[#(#transient),*];));
    let server = (!hidden.is_empty()).then(|| quote!(const SERVER: &'static [&'static str] = &[#(#hidden),*];));
    let tables = (!tables.is_empty()).then(|| {
        quote!(const TABLES: &'static [(&'static str, &'static str, ::std::option::Option<&'static str>, ::std::option::Option<&'static str>)] = &[#(#tables),*];)
    });
    // aio files are a native store's: a browser build has none.
    let slices = (!slices.is_empty()).then(
        || quote!(#[cfg(not(target_arch = "wasm32"))] const AIO_CELLS: &'static [::aio::compat::data::Slice] = &[#(#slices),*];),
    );
    let save = f.save.map(|p| quote!(fn save(&self) -> ::std::option::Option<Self> { Some(#p(self)) }));
    let restore = f.restore.map(|p| quote!(fn restore(self) -> Self { #p(self) }));
    let retire = &f.retire;
    let retire = (!retire.is_empty()).then(|| quote!(const RETIRED: &'static [&'static str] = &[#(#retire),*];));
    let default = (!f.custom_default).then(|| quote!(Default,));
    let coalesce = f.coalesce.then(|| {
        quote!(
            const COALESCE: bool = true;
        )
    });
    let quiet = f.quiet.then(|| {
        quote!(
            const DIAGNOSTICS: bool = false;
        )
    });
    let machine = match &f.machine {
        None => None,
        Some(field) => {
            let Some(ty) = s.fields.iter().find(|x| x.ident.as_ref() == Some(field)).map(|x| &x.ty) else {
                let msg = format!("machine = {field}: no field `{field}`");
                return syn::Error::new(field.span(), msg).to_compile_error().into();
            };
            Some(quote! {
                fn advance(&mut self, method: &str) -> ::aio::Result {
                    if let Some(next) = ::aio::machine::step(self.#field, method)? {
                        self.#field = next;
                    }
                    Ok(())
                }
                fn machine_errors(methods: &[&str]) -> ::std::vec::Vec<::std::string::String> {
                    ::aio::machine::errors::<#ty>(methods)
                }
                fn machine_ends() -> ::std::vec::Vec<::std::string::String> {
                    ::aio::machine::dead_ends::<#ty>().iter().map(|s| format!("{s:?}")).collect()
                }
            })
        }
    };
    let project = (!hidden.is_empty() || f.visible.is_some()).then(|| {
        let filtered = match &f.visible {
            Some(v) => quote!(::aio::serde_json::to_value(::aio::visible(#v, self, viewer))),
            None => quote!(::aio::serde_json::to_value(self)),
        };
        quote! {
            fn project(&self, viewer: &::aio::Caller) -> ::aio::serde_json::Value {
                let _ = viewer;
                let mut v = #filtered.unwrap_or(::aio::serde_json::Value::Null);
                if let Some(m) = v.as_object_mut() {
                    #(m.remove(#hidden);)*
                }
                v
            }
        }
    });
    // Each field's place, and which differ: a view re-renders for its own.
    let members: Vec<syn::Member> = s
        .fields
        .iter()
        .enumerate()
        .map(|(i, x)| match &x.ident {
            Some(id) => syn::Member::Named(id.clone()),
            None => syn::Member::Unnamed(syn::Index::from(i)),
        })
        .collect();
    let field_names = members.iter().map(|m| match m {
        syn::Member::Named(id) => id.to_string(),
        syn::Member::Unnamed(i) => i.index.to_string(),
    });
    let field_tys = s.fields.iter().map(|x| &x.ty);
    let bits = (0..members.len()).map(|i| i.min(63) as u32);
    let fields = quote! {
        const FIELDS: &'static [::aio::Field] = &[#(::aio::Field {
            name: #field_names,
            at: ::core::mem::offset_of!(Self, #members),
            size: ::core::mem::size_of::<#field_tys>(),
        }),*];
        fn changed_fields(&self, other: &Self) -> u64 {
            let mut m = 0u64;
            #(if self.#members != other.#members { m |= 1u64 << #bits; })*
            m
        }
    };
    quote! {
        #[derive(Clone, Debug, #default PartialEq, ::aio::serde::Serialize, ::aio::serde::Deserialize)]
        #[serde(crate = "::aio::serde", default)]
        #s

        impl ::aio::CellData for #name {
            const ID: &'static str = #id;
            const VERSION: u32 = #version;
            const PERSIST: bool = #persist;
            const SHARED: bool = #shared;
            #quiet
            #coalesce
            #machine
            #migrate
            #project
            #transient
            #server
            #tables
            #slices
            #save
            #restore
            #retire
            #fields
        }
    }
    .into()
}

/// A field name's words: `apiKey`, `api_key` → api, key; `SEED_2` → seed, 2.
fn words(name: &str) -> Vec<String> {
    let (mut out, mut w, mut prev) = (vec![], String::new(), None::<char>);
    for c in name.chars() {
        let cut = match prev {
            _ if !c.is_ascii_alphanumeric() => {
                out.push(std::mem::take(&mut w));
                prev = None;
                continue;
            }
            Some(p) => (p.is_ascii_lowercase() && c.is_ascii_uppercase()) || (p.is_ascii_digit() != c.is_ascii_digit()),
            None => false,
        };
        if cut {
            out.push(std::mem::take(&mut w));
        }
        w.push(c.to_ascii_lowercase());
        prev = Some(c);
    }
    out.push(w);
    out.into_iter().filter(|w| !w.is_empty()).collect()
}

/// An unambiguous credential's name (aio's `isRefusableCredential`): a
/// password, passphrase or mnemonic; a private, API or secret key; an
/// access or auth token. Not a public one, nor what is only about one
/// (`password_at`, `api_key_id`).
fn credential(name: &str) -> bool {
    const ABOUT: &[&str] = &[
        "id", "ids", "type", "name", "count", "index", "idx", "at", "ref", "kind", "length", "len", "path", "mode",
        "status", "flag", "enabled", "visible", "label", "order", "version", "ms", "sec", "secs", "seconds", "bytes",
        "kb", "mb", "gb", "hz", "pct", "percent", "ratio", "rate", "total", "avg", "min", "max", "size", "width",
        "height", "duration", "elapsed",
    ];
    let w = words(name);
    if w.iter().any(|x| x == "pub" || x == "public") || w.last().is_some_and(|x| ABOUT.contains(&x.as_str())) {
        return false;
    }
    let pairs = [("private", "key"), ("api", "key"), ("secret", "key"), ("access", "token"), ("auth", "token")];
    w.iter().any(|x| ["password", "passwrd", "passphrase", "mnemonic"].contains(&x.as_str()))
        || w.windows(2).any(|p| pairs.contains(&(p[0].as_str(), p[1].as_str())))
}

fn snake(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

fn take(attrs: &mut Vec<Attribute>, name: &str) -> Option<Attribute> {
    let i = attrs.iter().position(|a| a.path().is_ident(name))?;
    Some(attrs.remove(i))
}

fn is_cx(t: &Type) -> bool {
    let t = if let Type::Reference(r) = t { &*r.elem } else { t };
    matches!(t, Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "Cx"))
}

/// How a parameter is decoded and handed to the method: `&str` → String,
/// `&[T]` → Vec<T>, `&T` → T (passed borrowed), `Option<&…>` likewise
/// inside the option. Other borrowed shapes can't be decoded.
enum Pass {
    Value,
    Ref,
    OptDeref,
    OptRef,
}

fn deref_owned(e: &Type) -> (Type, bool) {
    match e {
        Type::Path(p) if p.path.is_ident("str") => (parse_quote!(::std::string::String), true),
        Type::Slice(s) => {
            let el = &s.elem;
            (parse_quote!(::std::vec::Vec<#el>), true)
        }
        _ => (e.clone(), false),
    }
}

fn owned(t: &Type) -> syn::Result<(Type, Pass)> {
    if let Type::Reference(r) = t {
        return Ok((deref_owned(&r.elem).0, Pass::Ref));
    }
    if let Type::Path(p) = t
        && let Some(seg) = p.path.segments.last()
        && seg.ident == "Option"
        && let PathArguments::AngleBracketed(a) = &seg.arguments
        && let Some(GenericArgument::Type(Type::Reference(r))) = a.args.first()
    {
        let (inner, deref) = deref_owned(&r.elem);
        return Ok((parse_quote!(::std::option::Option<#inner>), if deref { Pass::OptDeref } else { Pass::OptRef }));
    }
    if has_ref(t) {
        return Err(syn::Error::new(t.span(), "a cell method can't take this borrowed type: take it owned"));
    }
    Ok((t.clone(), Pass::Value))
}

fn has_ref(t: &Type) -> bool {
    quote!(#t).into_iter().any(|tt| matches!(tt, proc_macro2::TokenTree::Punct(p) if p.as_char() == '&'))
}

/// `Result<R>` → R, `Result` → (), no return → None (the method can't refuse).
fn ok_type(r: &ReturnType) -> syn::Result<Option<Type>> {
    let ReturnType::Type(_, t) = r else { return Ok(None) };
    if let Type::Path(p) = &**t
        && let Some(seg) = p.path.segments.last()
        && seg.ident == "Result"
    {
        return Ok(Some(match &seg.arguments {
            PathArguments::AngleBracketed(a) => match a.args.first() {
                Some(GenericArgument::Type(t)) => t.clone(),
                _ => parse_quote!(()),
            },
            _ => parse_quote!(()),
        }));
    }
    Err(syn::Error::new(t.span(), "a cell method returns aio::Result<T> (or nothing)"))
}

/// A value an async method keeps across an await: an argument, a typed
/// `let`, or an awaited result.
#[derive(Clone)]
struct Var {
    name: Ident,
    pat: Pat,
    ty: Type,
}

/// An async method's code up to an await, and that await: the future and
/// where its result goes.
type Part = (Vec<Stmt>, Option<(Expr, Var)>);

fn var(pat: &Pat, ty: &Type) -> Option<Var> {
    let Pat::Ident(p) = pat else { return None };
    Some(Var { name: p.ident.clone(), pat: pat.clone(), ty: ty.clone() })
}

/// `pub async fn m(&mut self, cx: &Cx, …) -> Result`, split at each top-level
/// `let x: T = io.await;`: the code before the first await stays the method,
/// each await becomes an effect started after that part commits, and the
/// code after it a step method (`m.1`, `m.2`, …) the effect calls with `x`.
/// Each part is one atomic call and the log holds every awaited result, so
/// replay does no I/O. Only arguments and typed `let`s live across an await.
///
/// `#[ttl(ms)]` on it: a call with the same arguments within `ms` of one whose
/// every part committed is answered `Ok(())` without running (no event, no
/// I/O; aio's `ttl`). Its effects are then keyed by the arguments too, so
/// `fetch(1)` and `fetch(2)` neither share an answer nor wait for each other.
fn split(f: &mut ImplItemFn, ttl: Option<i64>, long: bool) -> syn::Result<Vec<(ImplItemFn, Tokens)>> {
    let err = |s: &dyn Spanned, msg: &str| Err(syn::Error::new(s.span(), msg));
    let m = f.sig.ident.clone();
    if !matches!(ok_type(&f.sig.output)?, Some(Type::Tuple(t)) if t.elems.is_empty()) {
        return err(&f.sig, "an async method returns aio::Result: a step after an await may refuse");
    }
    let (mut cx, mut known) = (None, vec![]);
    let mut first = vec![];
    for (i, a) in f.sig.inputs.iter().skip(1).enumerate() {
        let FnArg::Typed(pt) = a else { continue };
        if i == 0 && is_cx(&pt.ty) {
            let Pat::Ident(p) = &*pt.pat else { return err(pt, "name the `Cx` argument") };
            cx = Some(p.ident.clone());
            continue;
        }
        if has_ref(&pt.ty) {
            return err(&pt.ty, "an async method takes its arguments owned: they live across awaits");
        }
        match var(&pt.pat, &pt.ty) {
            Some(v) => {
                first.push(v.name.clone());
                known.push(v);
            }
            None => return err(&pt.pat, "name each argument of an async method"),
        }
    }
    let cx = cx.unwrap_or_else(|| {
        f.sig.inputs.insert(1, parse_quote!(__aio_cx: &::aio::Cx));
        format_ident!("__aio_cx")
    });
    // The parts between awaits, each with the await that ends it.
    let mut parts: Vec<Part> = vec![(vec![], None)];
    for s in std::mem::take(&mut f.block.stmts) {
        if let Stmt::Local(l) = &s
            && let Some(init) = &l.init
            && let Expr::Await(a) = &*init.expr
            && init.diverge.is_none()
        {
            let Pat::Type(pt) = &l.pat else { return err(&l.pat, "write `let x: T = io.await;`: T is how x is saved") };
            let Some(v) = var(&pt.pat, &pt.ty) else {
                return err(&pt.pat, "an awaited result goes into one named `let`");
            };
            parts.last_mut().expect("a part").1 = Some(((*a.base).clone(), v));
            parts.push((vec![], None));
        } else if scan(quote!(#s), &mut vec![]) {
            return err(&s, "in an async method, `.await` goes only in `let x: T = io.await;`");
        } else {
            parts.last_mut().expect("a part").0.push(s);
        }
    }
    if ttl.is_some() && parts.len() == 1 {
        return err(&f.sig, "#[ttl] caches what an async method awaits: this one awaits nothing");
    }
    // The names used from each part on.
    let mut later = vec![vec![]; parts.len() + 1];
    for (k, (stmts, end)) in parts.iter().enumerate().rev() {
        let mut names = later[k + 1].clone();
        let end = end.as_ref().map(|(io, _)| io);
        scan(quote!(#(#stmts)* #end), &mut names);
        later[k] = names;
    }
    let (mut steps, mut args, last) = (vec![], vec![], parts.len() - 1);
    for (k, (stmts, end)) in parts.into_iter().enumerate() {
        for s in &stmts {
            if let Stmt::Local(l) = s
                && let Pat::Type(pt) = &l.pat
            {
                known.extend(var(&pt.pat, &pt.ty));
            }
        }
        let body = match end {
            None => quote!(#(#stmts)*),
            Some((io, r)) => {
                // The latest of each name the next parts use, then the result.
                let mut next: Vec<Var> = vec![];
                for x in known.iter().rev() {
                    let n = x.name.to_string();
                    if x.name != r.name && later[k + 1].contains(&n) && !next.iter().any(|y| y.name == x.name) {
                        next.insert(0, x.clone());
                    }
                }
                next.push(r.clone());
                known.push(r);
                let (step, key, names) = (format!("{m}.{}", k + 1), m.to_string(), next.iter().map(|x| &x.name));
                let (r, rt) = (&next[next.len() - 1].name, &next[next.len() - 1].ty);
                args.push(next.clone());
                // With a ttl: the first part keys its effect by the arguments,
                // and each step passes on the key its effect had.
                let keyed = match (ttl, k) {
                    (None, _) => quote!(let __aio_key = ::std::string::String::from(#key);),
                    (Some(_), 0) => quote! {
                        let __aio_key = ::aio::effect::key(#key, &::aio::json(&(#(&#first,)*))?)?;
                    },
                    (Some(_), _) => quote! {
                        let __aio_key = match #cx.by() {
                            ::aio::By::Effect(k) => k.clone(),
                            _ => ::std::string::String::from(#key),
                        };
                    },
                };
                let ceiling = (!long).then(|| quote!(.timeout(::aio::effect::CALL_CEILING)));
                let cache = match ttl.filter(|_| k + 1 == last) {
                    Some(ms) => quote!(Some(#ms)),
                    None => quote!(::std::option::Option::None),
                };
                quote! {
                    #keyed
                    #(#stmts)*
                    let __aio_io = #io;
                    #cx.run(::aio::Effect::new(&__aio_key, move |__aio_h: ::aio::Handle<Self>| async move {
                        let #r: #rt = __aio_io.await;
                        ::aio::step(&__aio_h, #step, ::aio::json(&(#(#names,)*)), #cache);
                    })#ceiling);
                    Ok(())
                }
            }
        };
        if k == 0 {
            // A `let mut` may change only in a later step.
            f.attrs.push(parse_quote!(#[allow(unused_mut)]));
            f.sig.asyncness = None;
            f.block = parse_quote!({ #body });
            continue;
        }
        let (name, fun, vars) = (format!("{m}.{k}"), format_ident!("__aio_{m}_{k}"), &args[k - 1]);
        let (pats, names, tys) =
            (vars.iter().map(|x| &x.pat), vars.iter().map(|x| &x.name), vars.iter().map(|x| &x.ty));
        let (names2, tys2) = (names.clone(), tys.clone());
        steps.push((
            parse_quote! {
                #[doc(hidden)]
                #[allow(unused_mut, unused_variables, clippy::too_many_arguments)]
                fn #fun(&mut self, #cx: &::aio::Cx, #(#pats: #tys),*) -> ::aio::Result { #body }
            },
            quote! {
                #name => {
                    let (#(#names,)*): (#(#tys2,)*) = ::aio::args(args)?;
                    ::aio::ret(self.#fun(cx, #(#names2),*))
                }
            },
        ));
    }
    Ok(steps)
}

/// The names `t` uses (not `.field`s; `{name}`s in format strings too), and
/// whether it awaits (outside its own `async` blocks).
fn scan(t: Tokens, names: &mut Vec<String>) -> bool {
    let (mut dot, mut asyncb, mut awaits) = (false, false, false);
    for tt in t {
        match &tt {
            TokenTree::Group(g) => awaits |= scan(g.stream(), names) && !asyncb,
            TokenTree::Ident(i) if dot => awaits |= i == "await",
            TokenTree::Ident(i) => names.push(i.to_string()),
            TokenTree::Literal(l) => names.extend(
                l.to_string()
                    .split('{')
                    .skip(1)
                    .map(|p| p.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect()),
            ),
            TokenTree::Punct(_) => {}
        }
        dot = matches!(&tt, TokenTree::Punct(p) if p.as_char() == '.');
        asyncb = matches!(&tt, TokenTree::Ident(i) if i == "async" || (asyncb && i == "move"));
    }
    awaits
}

/// `#[methods] impl Todo { pub fn add(&mut self, cx: &Cx, title: String) -> Result<Id> … }`.
/// `#[invariant] fn …(&self) -> Result` runs after every method; `#[client]`
/// marks the methods clients may call over the network (see `client_rule`:
/// `role = "admin"`, `allow = f`, `predict = false`); `#[redact]` keeps a
/// method's arguments off disk and out of `ax events` (its resulting state
/// is logged instead) and its cell's state out of problem reports;
/// `#[coalesce]` logs a run of a method's calls as one event (a cursor's
/// steps: see `aio::Cell::COALESCED`); `#[derived] fn
/// …(&self, …) -> T` is a derived value (aio selector), cached through
/// `<Name>Derived` per state, or with `#[derived(reads = [rows, prices])]`
/// while those fields stay the same (aio's `memoLast`). A `pub async fn`
/// awaits I/O inline, and `#[ttl(ms)]` reuses its answer (see `split`).
/// `#[every("5m")]` or `#[cron("0 9 * * 1")]` (`, skip_if_running`) on a
/// method without arguments runs it on that schedule (`aio::Declared`).
/// `aio::Handle`'s own `&self` methods (native and web): a cell method
/// named like one is shadowed on a handle, so `#[methods]` refuses it.
const HANDLE_METHODS: &[&str] = &[
    "busy",
    "by",
    "call_from",
    "closing",
    "done",
    "enabled",
    "events",
    "listen",
    "listens_to",
    "on_change",
    "on_pending",
    "owns",
    "pause",
    "paused",
    "pending",
    "shutdown",
    "snapshot",
    "start",
    "state",
    "store",
    "tells",
    "travel",
    "until",
    "watch",
];

#[proc_macro_attribute]
pub fn methods(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut imp = parse_macro_input!(item as ItemImpl);
    match expand(&mut imp) {
        Ok(t) => t.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(imp: &mut ItemImpl) -> syn::Result<Tokens> {
    let ty = imp.self_ty.clone();
    let Type::Path(tp) = &*ty else { return Err(syn::Error::new(ty.span(), "#[methods] goes on `impl Cell`")) };
    let calls = format_ident!("{}Calls", tp.path.segments.last().expect("type name").ident);
    let derived_trait = format_ident!("{}Derived", tp.path.segments.last().expect("type name").ident);
    let (mut arms, mut names, mut clients, mut invs, mut sigs, mut bodies) =
        (vec![], vec![], vec![], vec![], vec![], vec![]);
    let (mut dsigs, mut dbodies) = (vec![], vec![]);
    let (mut steps, mut rules, mut unpredicted, mut redacted, mut said) = (vec![], vec![], vec![], vec![], vec![]);
    let (mut schedules, mut coalesced) = (vec![], vec![]);
    let (mut on_start, mut on_stop, mut on_aio) = (None, None, None);
    for it in &mut imp.items {
        let ImplItem::Fn(f) = it else { continue };
        if let Some(d) = take(&mut f.attrs, "derived") {
            let (sig, body) = derive_one(&ty, &f.sig, &d)?;
            dsigs.push(sig);
            dbodies.push(body);
            continue;
        }
        let inv = take(&mut f.attrs, "invariant").is_some();
        let ttl = match take(&mut f.attrs, "ttl") {
            Some(a) if f.sig.asyncness.is_none() => {
                return Err(syn::Error::new(a.span(), "#[ttl(ms)] goes on a `pub async fn` method"));
            }
            Some(a) => {
                let ms: i64 = a.parse_args::<LitInt>()?.base10_parse()?;
                if ms <= 0 {
                    return Err(syn::Error::new(a.span(), "#[ttl(ms)]: ms must be > 0"));
                }
                Some(ms)
            }
            None => None,
        };
        let client = take(&mut f.attrs, "client");
        let redact = take(&mut f.attrs, "redact");
        let coalesce = take(&mut f.attrs, "coalesce");
        let long = take(&mut f.attrs, "long");
        if let Some(a) = &long
            && f.sig.asyncness.is_none()
        {
            return Err(syn::Error::new(a.span(), "#[long] goes on a `pub async fn` method"));
        }
        let hooks = [("on_start", &mut on_start), ("on_stop", &mut on_stop), ("on_aio", &mut on_aio)];
        let hook = hooks.into_iter().find_map(|(h, slot)| take(&mut f.attrs, h).map(|a| (h, a, slot)));
        let timed = ["every", "cron"].into_iter().find_map(|h| take(&mut f.attrs, h).map(|a| (h, a)));
        let m = f.sig.ident.clone();
        let scheduled = timed.is_some();
        if inv {
            invs.push(m);
            continue;
        }
        let callable = matches!(f.vis, Visibility::Public(_))
            && matches!(f.sig.inputs.first(), Some(FnArg::Receiver(r)) if r.reference.is_some() && r.mutability.is_some());
        if let Some((h, a, slot)) = hook {
            let args = f.sig.inputs.iter().skip(1).filter(|a| !matches!(a, FnArg::Typed(pt) if is_cx(&pt.ty))).count();
            if !callable || args > 0 || f.sig.asyncness.is_some() {
                let why = format!("#[{h}] goes on a `pub fn (&mut self)` method (a `cx: &Cx` may follow)");
                return Err(syn::Error::new(a.span(), why));
            }
            if slot.is_some() {
                return Err(syn::Error::new(a.span(), format!("a cell has one #[{h}] method")));
            }
            *slot = Some(m.to_string());
        }
        if let Some((h, a)) = timed {
            let args = f.sig.inputs.iter().skip(1).filter(|a| !matches!(a, FnArg::Typed(pt) if is_cx(&pt.ty))).count();
            if !callable || args > 0 {
                let why = format!("#[{h}] goes on a `pub fn (&mut self)` method (a `cx: &Cx` may follow)");
                return Err(syn::Error::new(a.span(), why));
            }
            let (every, cron, skip) = schedule(h, &a)?;
            let name = m.to_string();
            schedules
                .push(quote!(::aio::Declared { method: #name, every: #every, cron: #cron, skip_if_running: #skip }));
        }
        if !callable {
            if let Some(c) = client {
                return Err(syn::Error::new(c.span(), "#[client] goes on a `pub fn (&mut self, …)` method"));
            }
            if let Some(r) = redact {
                return Err(syn::Error::new(r.span(), "#[redact] goes on a `pub fn (&mut self, …)` method"));
            }
            if let Some(c) = coalesce {
                return Err(syn::Error::new(c.span(), "#[coalesce] goes on a `pub fn (&mut self, …)` method"));
            }
            if ttl.is_some() || long.is_some() {
                let what = if ttl.is_some() { "#[ttl(ms)]" } else { "#[long]" };
                return Err(syn::Error::new(f.sig.span(), format!("{what} goes on a `pub async fn` method")));
            }
            continue;
        }
        let mname = m.to_string();
        if mname.starts_with('$') || mname == "call" || mname == "read" {
            return Err(syn::Error::new(m.span(), "`call` and `read` are taken by aio::Door"));
        }
        if HANDLE_METHODS.contains(&mname.as_str()) {
            let cell = tp.path.segments.last().expect("type name").ident.to_string().to_lowercase();
            let why = format!(
                "`{mname}` is also `aio::Handle::{mname}`, which wins on a handle: `h.{mname}(..)` would \
                 call `Handle::{mname}`, never this method. Rename it (e.g. `{mname}_{cell}`)"
            );
            return Err(syn::Error::new(m.span(), why));
        }
        if f.sig.asyncness.is_some() {
            for (fun, arm) in split(f, ttl, long.is_some())? {
                steps.push(ImplItem::Fn(fun));
                arms.push(arm);
            }
        }
        if redact.is_some() {
            redacted.push(mname.clone());
        }
        if let Some(c) = &coalesce {
            // Its run is logged as its last call: an async step, a redacted
            // argument or a schedule would lose what the others did.
            if f.sig.asyncness.is_some() || redact.is_some() || scheduled {
                let why =
                    "#[coalesce] goes on a plain `pub fn (&mut self, …)` method: not async, #[redact] or scheduled";
                return Err(syn::Error::new(c.span(), why));
            }
            coalesced.push(mname.clone());
        }
        let (mut cx, mut vars, mut des, mut pass, mut params) = (None, vec![], vec![], vec![], vec![]);
        for (i, a) in f.sig.inputs.iter().skip(1).enumerate() {
            let FnArg::Typed(pt) = a else { continue };
            if i == 0 && is_cx(&pt.ty) {
                cx = Some(quote!(cx,));
                continue;
            }
            let v = match &*pt.pat {
                Pat::Ident(p) => p.ident.clone(),
                _ => format_ident!("a{i}"),
            };
            let (de, how) = owned(&pt.ty)?;
            let t = &pt.ty;
            pass.push(match how {
                Pass::Value => quote!(#v),
                Pass::Ref => quote!(&#v),
                Pass::OptDeref => quote!(#v.as_deref()),
                Pass::OptRef => quote!(#v.as_ref()),
            });
            params.push(quote!(#v: #t));
            des.push(de);
            vars.push(v);
        }
        let ok = ok_type(&f.sig.output)?;
        let run = quote!(self.#m(#cx #(#pass),*));
        let run =
            if ok.is_some() { quote!(::aio::ret(#run)) } else { quote!({ #run; Ok(::aio::serde_json::Value::Null) }) };
        arms.push(quote! {
            #mname => {
                let (#(#vars,)*): (#(#des,)*) = ::aio::args(args)?;
                #run
            }
        });
        let r = ok.unwrap_or_else(|| parse_quote!(()));
        let text = |t: Tokens| t.to_string().replace(' ', "").replace(',', ", ");
        let ps: Vec<String> = params.iter().map(|p| text(p.clone()).replacen(':', ": ", 1)).collect();
        said.push(format!("{mname}({}) -> {}", ps.join(", "), text(quote!(#r))));
        sigs.push(quote!(fn #m(&self, #(#params),*) -> ::aio::Result<#r>;));
        bodies.push(quote! {
            fn #m(&self, #(#params),*) -> ::aio::Result<#r> {
                ::aio::typed(self.call(#mname, ::aio::json(&(#(&#vars,)*))?))
            }
        });
        if let Some(c) = &client {
            let (role, allow, predict) = client_rule(c)?;
            if !predict {
                // Its answer comes from the server, later: the call itself returns nothing.
                if ok_type(&f.sig.output)?.is_some_and(|t| quote!(#t).to_string() != "()") {
                    return Err(syn::Error::new(
                        f.sig.output.span(),
                        "a predict = false method returns nothing: its answer comes from the server later (call_confirmed)",
                    ));
                }
                unpredicted.push(mname.clone());
            }
            let role = role.map(|r| {
                let why = format!("needs the role {}", r.value());
                quote!(if who.role.as_deref() != Some(#r) { return Err(::aio::Rejected::denied(method, #why)); })
            });
            let allow = allow.map(|p| {
                let rule = quote!(#p).to_string().replace(' ', "");
                quote! {
                    let (#(#vars,)*): (#(#des,)*) = ::aio::args(args.clone())?;
                    ::aio::allowed(method, #rule, || #p(self, who, #(#pass),*))?;
                }
            });
            rules.push(quote!(#mname => { #role #allow Ok(()) }));
            clients.push(mname.clone());
        }
        names.push(mname);
    }
    imp.items.extend(steps);
    let derived = (!dsigs.is_empty()).then(|| {
        quote! {
            /// The `#[derived]` values, cached: `state.f()` on an `Arc` of the
            /// cell (`h.state()`, a view's reader) computes once per state and
            /// arguments; on a plain `&Cell` it is the uncached fn.
            pub trait #derived_trait {
                #(#dsigs)*
            }

            impl #derived_trait for ::std::sync::Arc<#ty> {
                #(#dbodies)*
            }
        }
    });
    let allow = (!rules.is_empty()).then(|| {
        quote! {
            #[allow(unused_variables)]
            fn allow(&self, who: &::aio::Caller, method: &str, args: &::aio::serde_json::Value) -> ::aio::Result {
                match method {
                    #(#rules)*
                    _ => Err(::aio::Rejected::denied(method, "is not a #[client] method")),
                }
            }
        }
    });
    let on_start = on_start.map(|m| quote!(const ON_START: Option<&'static str> = Some(#m);));
    let on_stop = on_stop.map(|m| quote!(const ON_STOP: Option<&'static str> = Some(#m);));
    // `aio::compat::on_aio` builds only with aio-rs's feature `compat` (it says so).
    let aio_gate = on_aio.as_ref().map(|m| quote!(const _: &str = ::aio::compat::on_aio(#m);));
    let on_aio = on_aio.map(|m| quote!(const ON_AIO: Option<&'static str> = Some(#m);));
    Ok(quote! {
        #imp

        #aio_gate

        impl ::aio::Cell for #ty {
            #allow
            const METHODS: &'static [&'static str] = &[#(#names),*];
            const SIGNATURES: &'static [&'static str] = &[#(#said),*];
            const CLIENT: &'static [&'static str] = &[#(#clients),*];
            const UNPREDICTED: &'static [&'static str] = &[#(#unpredicted),*];
            const REDACTED: &'static [&'static str] = &[#(#redacted),*];
            const COALESCED: &'static [&'static str] = &[#(#coalesced),*];
            const SCHEDULES: &'static [::aio::Declared] = &[#(#schedules),*];
            #on_start
            #on_stop
            #on_aio
            #[allow(unused_variables)]
            fn apply(&mut self, cx: &::aio::Cx, method: &str, args: ::aio::serde_json::Value)
                -> ::aio::Result<::aio::serde_json::Value> {
                match method {
                    #(#arms)*
                    _ => Err(::aio::Rejected::unknown(method)),
                }
            }
            fn check(&self) -> ::aio::Result {
                #(self.#invs()?;)*
                Ok(())
            }
        }

        /// Typed calls through any door (a store handle, a UI client, a test).
        pub trait #calls {
            #(#sigs)*
        }

        impl<D: ::aio::Door<#ty> + ?Sized> #calls for D {
            #(#bodies)*
        }

        #derived
    })
}

/// `#[every(300_000)]`, `#[every("5m")]` (d, h, m, s) or `#[cron("0 9 * * 1")]`,
/// each optionally `, skip_if_running`: its `every`, `cron` and skip flag.
fn schedule(h: &str, a: &Attribute) -> syn::Result<(Tokens, Tokens, bool)> {
    let parts = a.parse_args_with(syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated)?;
    let mut it = parts.iter();
    let lit = |e: Option<&Expr>| match e {
        Some(Expr::Lit(l)) => Some(l.lit.clone()),
        _ => None,
    };
    let bad = |why: &str| Err(syn::Error::new(a.span(), format!("#[{h}]: {why}")));
    let (every, cron) = match (h, lit(it.next())) {
        ("every", Some(syn::Lit::Int(n))) => (n.base10_parse::<i64>()?, None),
        ("every", Some(syn::Lit::Str(s))) => match span_ms(&s.value()) {
            Some(ms) => (ms, None),
            None => return bad("a span is a number and d, h, m or s (\"5m\"), or ms"),
        },
        ("cron", Some(syn::Lit::Str(s))) => (0, Some(s.value())),
        ("every", _) => return bad("give the interval: #[every(\"5m\")] or #[every(300_000)]"),
        _ => return bad("give the expression: #[cron(\"0 9 * * 1\")]"),
    };
    if cron.is_none() && every <= 0 {
        return bad("the interval must be > 0");
    }
    let skip = match it.next() {
        None => false,
        Some(Expr::Path(p)) if p.path.is_ident("skip_if_running") && it.next().is_none() => true,
        Some(_) => return bad("after the timing, only `skip_if_running`"),
    };
    let every = if cron.is_some() { quote!(None) } else { quote!(Some(#every)) };
    let cron = match cron {
        Some(c) => quote!(Some(#c)),
        None => quote!(None),
    };
    Ok((every, cron, skip))
}

/// `5m` → ms (as `aio.toml` spans: d, h, m, s).
fn span_ms(s: &str) -> Option<i64> {
    let (n, unit) = s.trim().split_at(s.trim().find(|c: char| !c.is_ascii_digit())?);
    let n: i64 = n.parse().ok()?;
    let unit = match unit {
        "d" => 86_400_000,
        "h" => 3_600_000,
        "m" => 60_000,
        "s" => 1000,
        _ => return None,
    };
    n.checked_mul(unit)
}

/// `#[client]`: any client that got in (with the key, or signed in);
/// `role = "admin"`: only a session with that role now; `allow = f`
/// (`fn(&Self, &Caller, <the method's arguments>) -> bool`): as `f` says on
/// the state the call would run on (row-level rules). Both: both must pass.
/// `predict = false`: a synced copy never shows it before the server ran it
/// (payments, auth, ledgers: where a preview would lie).
fn client_rule(a: &Attribute) -> syn::Result<(Option<LitStr>, Option<Path>, bool)> {
    let (mut role, mut allow, mut predict) = (None, None, true);
    if matches!(a.meta, syn::Meta::Path(_)) {
        return Ok((role, allow, predict));
    }
    a.parse_nested_meta(|m| {
        if m.path.is_ident("role") {
            let r: LitStr = m.value()?.parse()?;
            // A string is a role name; these read like "everyone" or "no one".
            if ["", "none", "all", "true", "false", "public", "any", "*"].contains(&r.value().to_lowercase().as_str()) {
                return Err(syn::Error::new(r.span(), "role = names one role: for any client write #[client]"));
            }
            role = Some(r);
        } else if m.path.is_ident("allow") {
            allow = Some(m.value()?.parse()?);
        } else if m.path.is_ident("predict") {
            predict = m.value()?.parse::<syn::LitBool>()?.value;
        } else {
            return Err(m.error("unknown #[client] rule: expected role = \"…\", allow = f or predict = false"));
        }
        Ok(())
    })?;
    Ok((role, allow, predict))
}

/// One `#[derived] fn f(&self, …) -> R`: its cached twin on `Arc<Cell>`
/// (what handles, copies and views hold). The key is the state's `Arc` (with
/// `#[derived(reads = [a, b])]`: its fields `a` and `b`, aio memoLast's deps,
/// so a commit elsewhere, a cursor step, keeps the answer), a
/// `&Arc<Other>` parameter's `Arc` (a cell it reads), and every other
/// argument's owned value.
fn derive_one(ty: &Type, sig: &syn::Signature, attr: &Attribute) -> syn::Result<(Tokens, Tokens)> {
    let at = attr.span();
    let mut reads: Option<Vec<syn::Member>> = None;
    if !matches!(attr.meta, syn::Meta::Path(_)) {
        attr.parse_nested_meta(|m| {
            if !m.path.is_ident("reads") {
                return Err(m.error("expected #[derived] or #[derived(reads = [field, …])]"));
            }
            let list: syn::ExprArray = m.value()?.parse()?;
            let fields = list.elems.iter().map(|e| match e {
                Expr::Path(p) if p.path.get_ident().is_some() => {
                    Ok(syn::Member::Named(p.path.get_ident().expect("an ident").clone()))
                }
                e => Err(syn::Error::new(e.span(), "reads = [field, …]: the state's fields it reads")),
            });
            reads = Some(fields.collect::<syn::Result<_>>()?);
            Ok(())
        })?;
    }
    if !matches!(sig.inputs.first(), Some(FnArg::Receiver(r)) if r.reference.is_some() && r.mutability.is_none()) {
        return Err(syn::Error::new(at, "#[derived] goes on a `fn (&self, …) -> T`: it reads, never changes"));
    }
    let ReturnType::Type(_, r) = &sig.output else {
        return Err(syn::Error::new(at, "a #[derived] fn returns its value"));
    };
    if has_ref(r) {
        return Err(syn::Error::new(r.span(), "a #[derived] value is kept: return it owned"));
    }
    let m = &sig.ident;
    let (mut params, mut vars, mut kinds, mut keys) = (vec![], vec![], vec![], vec![]);
    for (i, a) in sig.inputs.iter().skip(1).enumerate() {
        let FnArg::Typed(pt) = a else { continue };
        let v = match &*pt.pat {
            Pat::Ident(p) => p.ident.clone(),
            _ => format_ident!("a{i}"),
        };
        let t = &pt.ty;
        let (kind, key) = match &**t {
            Type::Reference(rf) => match arc_of(&rf.elem) {
                Some(cell) => (quote!(::aio::Ver<#cell>), quote!(::aio::Ver::of(#v))),
                None => {
                    let e = &rf.elem;
                    (quote!(<#e as ::std::borrow::ToOwned>::Owned), quote!(::std::borrow::ToOwned::to_owned(#v)))
                }
            },
            _ if has_ref(t) => {
                return Err(syn::Error::new(t.span(), "a #[derived] fn can't take this borrowed type: take it owned"));
            }
            _ => (quote!(#t), quote!(::std::clone::Clone::clone(&#v))),
        };
        params.push(quote!(#v: #t));
        vars.push(v);
        kinds.push(kind);
        keys.push(key);
    }
    let (state_kind, state_key) = match &reads {
        None => (quote!(::aio::Ver<#ty>), quote!(::aio::Ver::of(self))),
        Some(f) => (quote!(::aio::Reads<#ty>), quote!(::aio::Reads::of(self, |a, b| true #(&& a.#f == b.#f)*))),
    };
    Ok((
        quote!(fn #m(&self, #(#params),*) -> #r;),
        quote! {
            fn #m(&self, #(#params),*) -> #r {
                static MEMO: ::aio::Derived<(#state_kind, #(#kinds,)*), #r> = ::aio::Derived::new();
                MEMO.get((#state_key, #(#keys,)*), || <#ty>::#m(self, #(#vars),*))
            }
        },
    ))
}

/// `Arc<T>` → T.
fn arc_of(t: &Type) -> Option<&Type> {
    let Type::Path(p) = t else { return None };
    let seg = p.path.segments.last().filter(|s| s.ident == "Arc")?;
    let PathArguments::AngleBracketed(a) = &seg.arguments else { return None };
    match a.args.first() {
        Some(GenericArgument::Type(t)) => Some(t),
        _ => None,
    }
}

/// `#[aio::data]` on a struct or enum a cell keeps (a field's type, a
/// method's argument): it is saved and sent, so it derives serde's
/// `Serialize` and `Deserialize` through aio's own serde (the app needs no
/// serde dependency). Other derives are the app's:
/// `#[aio::data] #[derive(Clone, Debug, Default, PartialEq)] pub struct Note { .. }`.
#[proc_macro_attribute]
pub fn data(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        let e = syn::Error::new(Tokens::from(attr).span(), "#[aio::data] takes nothing");
        return e.to_compile_error().into();
    }
    let item = Tokens::from(item);
    quote! {
        #[derive(::aio::serde::Serialize, ::aio::serde::Deserialize)]
        #[serde(crate = "::aio::serde")]
        #item
    }
    .into()
}

/// `#[derive(Lit)]` on an `aio.toml` type (in aio-config): it writes itself
/// as the Rust expression that makes it (`aio::config::Lit`), so an app's
/// config is parsed when the app is built, never when it runs. Its path is
/// named from `aio`, which re-exports each module under the same name.
#[proc_macro_derive(Lit)]
pub fn lit(item: TokenStream) -> TokenStream {
    lit_impl(&parse_macro_input!(item as syn::DeriveInput)).into()
}

fn lit_impl(input: &syn::DeriveInput) -> Tokens {
    let name = &input.ident;
    let at = quote!(concat!(module_path!(), "::", stringify!(#name)));
    let arms: Vec<Tokens> = match &input.data {
        syn::Data::Struct(s) => vec![lit_arm(quote!(Self), at, &s.fields)],
        syn::Data::Enum(e) => e
            .variants
            .iter()
            .map(|v| {
                let id = &v.ident;
                lit_arm(quote!(Self::#id), quote!(concat!(#at, "::", stringify!(#id))), &v.fields)
            })
            .collect(),
        syn::Data::Union(u) => {
            return syn::Error::new(u.union_token.span, "#[derive(Lit)]: not on a union").into_compile_error();
        }
    };
    quote! {
        impl crate::config::Lit for #name {
            fn lit(&self, o: &mut String) {
                match self { #(#arms)* }
            }
        }
    }
}

/// One shape's match arm: its path, then its fields as `{a:…,}` or `(…,)`.
fn lit_arm(pat: Tokens, path: Tokens, fields: &syn::Fields) -> Tokens {
    let binds: Vec<Ident> = (0..fields.len()).map(|i| format_ident!("f{i}")).collect();
    let names: Vec<Option<&Ident>> = fields.iter().map(|f| f.ident.as_ref()).collect();
    let labels = names.iter().map(|n| n.map(|n| format!("{n}: ")).unwrap_or_default());
    let (bind, open, close) = match fields {
        syn::Fields::Named(_) => (quote!({ #(#names: #binds),* }), "{", "}"),
        syn::Fields::Unnamed(_) => (quote!((#(#binds),*)), "(", ")"),
        syn::Fields::Unit => (quote!(), "", ""),
    };
    quote! {
        #pat #bind => {
            let path: &str = #path;
            o.push_str("aio");
            o.push_str(&path[path.find("::").unwrap_or(path.len())..]);
            o.push_str(#open);
            #( o.push_str(#labels); crate::config::Lit::lit(#binds, o); o.push(','); )*
            o.push_str(#close);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HANDLE_METHODS, credential, expand, span_ms, words};
    use syn::parse_quote;

    fn refused(mut imp: syn::ItemImpl) -> String {
        expand(&mut imp).err().map(|e| e.to_string()).unwrap_or_default()
    }

    #[test]
    fn a_method_named_like_a_handle_method_is_refused() {
        let r = refused(parse_quote!(impl S { pub fn start(&mut self, n: u32) {} }));
        assert!(r.contains("`start` is also `aio::Handle::start`") && r.contains("Rename it"), "{r}");
        let r = refused(parse_quote!(impl S { pub async fn travel(&mut self) -> Result { Ok(()) } }));
        assert!(r.contains("`travel` is also"), "{r}");
        assert!(
            refused(parse_quote!(impl S { fn state(&mut self) {} })).is_empty(),
            "a private fn isn't on the handle"
        );
        assert!(refused(parse_quote!(impl S { pub fn begin(&mut self) {} })).is_empty());
    }

    /// The list is `Handle`'s `&self` methods, as its sources define them.
    #[test]
    fn the_handle_methods_list_matches_the_handle() {
        let mut found = std::collections::BTreeSet::new();
        for f in ["../aio/src/handle.rs", "../aio/src/web/handle.rs"] {
            let src = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(f)).unwrap();
            let body = src.split("\nimpl<T: Cell> Handle<T> {\n").nth(1).unwrap();
            let body = &body[..body.find("\n}\n").unwrap()];
            let fns: Vec<&str> = body.split("\n    pub ").skip(1).filter(|s| !s.starts_with("(crate)")).collect();
            for f in fns {
                let f = f.trim_start_matches("async ").trim_start_matches("fn ");
                let name = &f[..f.find(['(', '<']).unwrap()];
                let sig = &f[..f.find('{').unwrap()];
                if sig.contains("&self") {
                    found.insert(name.to_string());
                }
            }
        }
        let listed: std::collections::BTreeSet<String> = HANDLE_METHODS.iter().map(|s| s.to_string()).collect();
        assert_eq!(found, listed);
    }

    #[test]
    fn async_methods_refuse_what_cannot_be_split() {
        let r = refused(parse_quote!(impl S { pub async fn m(&mut self) -> Result<u32> { Ok(1) } }));
        assert!(r.contains("returns aio::Result"), "{r}");
        let r = refused(parse_quote!(impl S { pub async fn m(&mut self) { } }));
        assert!(r.contains("returns aio::Result"), "{r}");
        let r = refused(parse_quote!(impl S { pub async fn m(&mut self, q: &str) -> Result { Ok(()) } }));
        assert!(r.contains("arguments owned"), "{r}");
        let r = refused(parse_quote!(impl S { pub async fn m(&mut self) -> Result { let x = io().await; Ok(()) } }));
        assert!(r.contains("T is how x is saved"), "{r}");
        let r = refused(parse_quote!(impl S { pub async fn m(&mut self) -> Result { self.n = io().await; Ok(()) } }));
        assert!(r.contains("`.await` goes only in"), "{r}");
        let r = refused(parse_quote!(impl S { pub async fn m(&mut self) -> Result { if a { io().await; } Ok(()) } }));
        assert!(r.contains("`.await` goes only in"), "{r}");
    }

    #[test]
    fn ttl_goes_only_on_an_async_method_that_awaits() {
        let r = refused(parse_quote!(impl S { #[ttl(10)] pub fn m(&mut self) {} }));
        assert!(r.contains("goes on a `pub async fn`"), "{r}");
        let r = refused(parse_quote!(impl S { #[ttl(10)] async fn m(&mut self) -> Result { Ok(()) } }));
        assert!(r.contains("goes on a `pub async fn`"), "{r}");
        let r = refused(parse_quote!(impl S { #[ttl(0)] pub async fn m(&mut self) -> Result { Ok(()) } }));
        assert!(r.contains("must be > 0"), "{r}");
        let r = refused(parse_quote!(impl S { #[ttl(10)] pub async fn m(&mut self) -> Result { Ok(()) } }));
        assert!(r.contains("awaits nothing"), "{r}");
    }

    #[test]
    fn long_goes_only_on_an_async_method_and_lifts_the_ceiling() {
        let r = refused(parse_quote!(impl S { #[long] pub fn m(&mut self) {} }));
        assert!(r.contains("#[long] goes on a `pub async fn`"), "{r}");
        let src = |long: bool| {
            let mut imp: syn::ItemImpl = if long {
                parse_quote!(impl S { #[long] pub async fn m(&mut self) -> Result { let n: u32 = io().await; Ok(()) } })
            } else {
                parse_quote!(impl S { pub async fn m(&mut self) -> Result { let n: u32 = io().await; Ok(()) } })
            };
            expand(&mut imp).unwrap().to_string()
        };
        assert!(src(false).contains("CALL_CEILING"));
        assert!(!src(true).contains("CALL_CEILING"));
    }

    #[test]
    fn a_hook_is_one_plain_method_without_arguments() {
        let r = refused(parse_quote!(impl S { #[on_start] pub fn m(&mut self, n: u32) {} }));
        assert!(r.contains("#[on_start] goes on a `pub fn (&mut self)`"), "{r}");
        let r = refused(parse_quote!(impl S { #[on_stop] fn m(&mut self) {} }));
        assert!(r.contains("#[on_stop] goes on"), "{r}");
        let r = refused(parse_quote!(impl S { #[on_start] pub fn a(&mut self) {} #[on_start] pub fn b(&mut self) {} }));
        assert!(r.contains("one #[on_start] method"), "{r}");
        let mut imp: syn::ItemImpl = parse_quote!(impl S { #[on_start] pub fn a(&mut self, cx: &Cx) {} });
        assert!(expand(&mut imp).unwrap().to_string().contains("ON_START"));
        let r = refused(parse_quote!(impl S { #[on_aio] pub fn m(&mut self, n: u32) {} }));
        assert!(r.contains("#[on_aio] goes on a `pub fn (&mut self)`"), "{r}");
        let mut imp: syn::ItemImpl = parse_quote!(impl S { #[on_aio] pub fn a(&mut self) -> Result {} });
        assert!(expand(&mut imp).unwrap().to_string().contains("ON_AIO"));
    }

    #[test]
    fn a_client_role_names_one_role() {
        for bad in ["none", "All", "true", "false", "public", "*", ""] {
            let r = refused(parse_quote!(impl S { #[client(role = #bad)] pub fn m(&mut self) {} }));
            assert!(r.contains("names one role"), "{bad}: {r}");
        }
        let r = refused(parse_quote!(impl S { #[client(roles = "a")] pub fn m(&mut self) {} }));
        assert!(r.contains("unknown #[client] rule"), "{r}");
        let r = refused(parse_quote!(impl S { #[client] fn m(&mut self) {} }));
        assert!(r.contains("#[client] goes on"), "{r}");
        let mut imp: syn::ItemImpl = parse_quote!(impl S { #[client(role = "admin")] pub fn m(&mut self) {} });
        assert!(expand(&mut imp).unwrap().to_string().contains("fn allow"));
    }

    #[test]
    fn a_declared_schedule_goes_on_a_method_without_arguments_and_says_its_timing() {
        let r = refused(parse_quote!(impl S { #[every("5m")] pub fn m(&mut self, n: u32) {} }));
        assert!(r.contains("#[every] goes on a `pub fn (&mut self)` method"), "{r}");
        let r = refused(parse_quote!(impl S { #[every("5 minutes")] pub fn m(&mut self) {} }));
        assert!(r.contains("a span is a number and d, h, m or s"), "{r}");
        let r = refused(parse_quote!(impl S { #[every(0)] pub fn m(&mut self) {} }));
        assert!(r.contains("must be > 0"), "{r}");
        let r = refused(parse_quote!(impl S { #[cron(5)] pub fn m(&mut self) {} }));
        assert!(r.contains("give the expression"), "{r}");
        let r = refused(parse_quote!(impl S { #[every("1h", skip)] pub fn m(&mut self) {} }));
        assert!(r.contains("only `skip_if_running`"), "{r}");
        let mut imp: syn::ItemImpl = parse_quote!(impl S { #[every("2h", skip_if_running)] pub fn m(&mut self) {} });
        let out = expand(&mut imp).unwrap().to_string();
        assert!(out.contains("every : Some (7200000i64)") && out.contains("skip_if_running : true"), "{out}");
        assert_eq!((span_ms("90s"), span_ms("1d"), span_ms("m")), (Some(90_000), Some(86_400_000), None));
    }

    #[test]
    fn a_credential_is_known_by_its_words() {
        assert_eq!(words("apiKey"), ["api", "key"]);
        assert_eq!(words("private_key"), ["private", "key"]);
        assert_eq!(words("SEED_2"), ["seed", "2"]);
        for c in
            ["password", "user_password", "passphrase", "mnemonic", "api_key", "apiKey", "private_key", "authToken"]
        {
            assert!(credential(c), "{c}");
        }
        for ok in
            ["passwordless", "monkey", "keyboard", "token_list", "public_key", "pub_key", "api_key_id", "password_at"]
        {
            assert!(!credential(ok), "{ok}");
        }
    }

    #[test]
    fn coalesce_goes_only_on_a_plain_callable_method() {
        let r = refused(parse_quote!(impl S { #[coalesce] fn m(&mut self, i: u32) {} }));
        assert!(r.contains("#[coalesce] goes on a `pub fn (&mut self, …)` method"), "{r}");
        let r = refused(parse_quote!(impl S { #[coalesce] #[redact] pub fn m(&mut self, i: u32) {} }));
        assert!(r.contains("not async, #[redact] or scheduled"), "{r}");
        let mut imp: syn::ItemImpl = parse_quote!(impl S { #[coalesce] pub fn go(&mut self, i: u32) {} });
        assert!(expand(&mut imp).unwrap().to_string().contains("COALESCED : & 'static [& 'static str] = & [\"go\"]"));
    }

    #[test]
    fn redact_goes_only_on_a_callable_method() {
        let r = refused(parse_quote!(impl S { #[redact] fn m(&mut self, pass: String) {} }));
        assert!(r.contains("#[redact] goes on a `pub fn (&mut self, …)` method"), "{r}");
        let mut imp: syn::ItemImpl = parse_quote!(impl S { #[redact] pub fn m(&mut self, pass: String) {} });
        assert!(expand(&mut imp).unwrap().to_string().contains("REDACTED : & 'static [& 'static str] = & [\"m\"]"));
    }

    #[test]
    fn an_unpredicted_method_returns_nothing() {
        let r =
            refused(parse_quote!(impl S { #[client(predict = false)] pub fn m(&mut self) -> Result<u32> { Ok(1) } }));
        assert!(r.contains("returns nothing"), "{r}");
        let mut imp: syn::ItemImpl = parse_quote!(impl S {
            #[client(predict = false, role = "a")] pub fn pay(&mut self) -> Result<()> { Ok(()) }
            #[client] pub fn add(&mut self) {}
        });
        let out = expand(&mut imp).unwrap().to_string();
        assert!(out.contains("const UNPREDICTED : & 'static [& 'static str] = & [\"pay\"]"), "{out}");
    }

    #[test]
    fn plain_async_helpers_and_inner_async_blocks_stay_as_written() {
        let mut imp: syn::ItemImpl = parse_quote! {
            impl S {
                async fn helper(&self) -> u32 { io().await }
                pub async fn m(&mut self, cx: &Cx, q: String) -> Result {
                    cx.run(Effect::new("e", |h| async move { io().await; }));
                    let a: u32 = io(q).await;
                    Ok(())
                }
            }
        };
        let out = expand(&mut imp).unwrap().to_string();
        assert!(out.contains("async fn helper"), "{out}");
        assert!(out.contains("\"m.1\" =>"), "{out}");
        assert!(out.contains("METHODS : & 'static [& 'static str] = & [\"m\"]"), "{out}");
    }
}

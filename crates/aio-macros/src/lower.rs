//! A `pub async fn` method lowered into parts that each commit on their
//! own: the code up to an await runs in the call, the await becomes an
//! effect started after that part commits, and the code after it a step
//! method (`m.1`, `m.2`, … numbered in source order) the effect calls with
//! what the rest needs and the awaited result. The log holds every awaited
//! result, so replay does no I/O.
//!
//! An await may stand anywhere a statement does: in a branch of `if`,
//! `if let` or `match`, in a `loop` or `while` (with `break` and
//! `continue`), in a block. A branch's or loop's end is a join, run in the
//! same call as a hidden function (no event of its own). What lives across
//! an await is an argument or a typed `let` (its type says how it is saved).
use proc_macro2::{Ident, Span, TokenStream as Tokens, TokenTree};
use quote::{format_ident, quote};
use std::collections::HashSet;
use syn::{
    Block, Expr, FnArg, ImplItemFn, Label, Lifetime, Pat, Stmt, Type, parse_quote, spanned::Spanned,
    visit_mut::VisitMut,
};

type Names = HashSet<String>;

/// A value in scope: an argument or a `let`'s binding. `ty: None` (an
/// untyped `let`, a pattern's binding) can't live across an await.
#[derive(Clone)]
struct Var {
    name: Ident,
    pat: Pat,
    ty: Option<Type>,
    span: Span,
}

/// What comes after a list of statements: its last expression is the
/// method's answer, or a join (a hidden function) runs with these values.
#[derive(Clone)]
enum Cont {
    Return,
    Goto(Ident, Vec<Var>),
    /// `let r: T = <if/match/block>;`: each branch's last expression is
    /// `r`, then the join runs with these values and `r`.
    Bind(Ident, Vec<Var>, Box<Var>),
}

impl Cont {
    /// Where a branch goes once its value is bound.
    fn bound(&self) -> Cont {
        match self {
            Cont::Bind(f, vars, r) => {
                let mut v = vars.clone();
                v.push((**r).clone());
                Cont::Goto(f.clone(), v)
            }
            k => k.clone(),
        }
    }
}

struct Frame {
    label: Option<Lifetime>,
    brk: Cont,
    next: Cont,
}

/// The parts of one async method: its hidden functions, and the `apply`
/// arms of its steps.
pub(crate) struct Lowered {
    pub fns: Vec<ImplItemFn>,
    pub arms: Vec<Tokens>,
}

struct Lower {
    m: Ident,
    cx: Ident,
    r: Type,
    ttl: Option<i64>,
    long: bool,
    newest: bool,
    awaits: usize,
    joins: usize,
    loops: Vec<Frame>,
    out: Lowered,
}

fn err<T>(s: &dyn Spanned, msg: &str) -> syn::Result<T> {
    Err(syn::Error::new(s.span(), msg))
}

const FORMS: &str = "in an async method, `.await` goes in a statement of its own: `let x: T = io.await;` \
                     (or `…await?;`), `x = io.await;`, `io.await;`, or the method's last expression";

/// The names `t` uses (not `.field`s or `::paths`; `{name}`s in format
/// strings too), and whether it awaits (outside its own `async` blocks).
pub(crate) fn scan(t: Tokens, names: &mut Vec<String>) -> bool {
    let (mut dot, mut colons, mut asyncb, mut awaits) = (false, 0, false, false);
    for tt in t {
        match &tt {
            TokenTree::Group(g) => awaits |= scan(g.stream(), names) && !asyncb,
            TokenTree::Ident(i) if dot => awaits |= i == "await",
            TokenTree::Ident(_) if colons == 2 => {}
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
        colons = match &tt {
            TokenTree::Punct(p) if p.as_char() == ':' => colons + 1,
            _ => 0,
        };
        asyncb = matches!(&tt, TokenTree::Ident(i) if i == "async" || (asyncb && i == "move"));
    }
    awaits
}

fn names_of(t: Tokens, into: &mut Names) -> bool {
    let mut v = vec![];
    let a = scan(t, &mut v);
    into.extend(v);
    a
}

fn awaits(t: &impl quote::ToTokens) -> bool {
    scan(quote!(#t), &mut vec![])
}

/// The names a pattern binds (an upper-case one is a unit variant or a const, not a binding).
fn bound(p: &Pat, into: &mut Vec<Ident>) {
    match p {
        Pat::Ident(i) => {
            if !i.ident.to_string().starts_with(|c: char| c.is_uppercase()) {
                into.push(i.ident.clone());
            }
            if let Some((_, sub)) = &i.subpat {
                bound(sub, into);
            }
        }
        Pat::Tuple(t) => t.elems.iter().for_each(|p| bound(p, into)),
        Pat::TupleStruct(t) => t.elems.iter().for_each(|p| bound(p, into)),
        Pat::Struct(s) => s.fields.iter().for_each(|f| bound(&f.pat, into)),
        Pat::Slice(s) => s.elems.iter().for_each(|p| bound(p, into)),
        Pat::Reference(r) => bound(&r.pat, into),
        Pat::Or(o) => o.cases.iter().take(1).for_each(|p| bound(p, into)),
        Pat::Type(t) => bound(&t.pat, into),
        Pat::Paren(p) => bound(&p.pat, into),
        _ => {}
    }
}

fn untyped(p: &Pat) -> Vec<Var> {
    let mut v = vec![];
    bound(p, &mut v);
    v.into_iter().map(|name| Var { pat: parse_quote!(#name), span: name.span(), name, ty: None }).collect()
}

/// A `let`'s bindings: one typed name (`let x: T`), else untyped ones.
fn let_vars(p: &Pat) -> Vec<Var> {
    if let Pat::Type(pt) = p
        && let Pat::Ident(i) = &*pt.pat
        && i.subpat.is_none()
    {
        return vec![Var { name: i.ident.clone(), pat: (*pt.pat).clone(), ty: Some((*pt.ty).clone()), span: p.span() }];
    }
    untyped(p)
}

/// The latest of each name in `after`, in declaration order: what a part
/// hands on. An untyped one there is refused (it couldn't be saved).
fn live(scope: &[Var], after: &Names) -> syn::Result<Vec<Var>> {
    let mut seen: Vec<String> = vec![];
    let mut out = vec![];
    for v in scope.iter().rev() {
        let n = v.name.to_string();
        if seen.contains(&n) || !after.contains(&n) {
            continue;
        }
        seen.push(n.clone());
        let Some(_) = &v.ty else {
            let why = format!(
                "`{n}` is used after an await: give it a type (`let {n}: T = …;`), which says how it is saved \
                 with the step"
            );
            return Err(syn::Error::new(v.span, why));
        };
        out.push(v.clone());
    }
    out.reverse();
    Ok(out)
}

/// What an await statement does with its result.
enum Into {
    /// `let x: T = io.await;`, `x = io.await;` (x typed, in scope).
    Bind(Box<Var>),
    /// `io.await;`, `let _ = io.await;`.
    Drop,
    /// The method's last expression: its answer.
    Answer,
}

struct Await {
    io: Expr,
    into: Into,
    /// `…await?`: an `Err` ends the method there, said to its caller.
    tried: bool,
}

fn awaited(e: &Expr) -> Option<(Expr, bool)> {
    match e {
        Expr::Await(a) => Some(((*a.base).clone(), false)),
        Expr::Try(t) => match &*t.expr {
            Expr::Await(a) => Some(((*a.base).clone(), true)),
            _ => None,
        },
        _ => None,
    }
}

impl Lower {
    fn key(&self) -> Tokens {
        quote!(__aio_key)
    }

    /// Go on with `k`: a join's function, run next in this same call.
    fn goto(&self, k: &Cont) -> Tokens {
        match k.bound() {
            Cont::Return | Cont::Bind(..) => quote!(),
            Cont::Goto(f, vars) => {
                let names = vars.iter().map(|v| &v.name);
                let key = self.key();
                quote! {
                    return ::std::result::Result::Ok(::aio::Step::Goto(::std::boxed::Box::new(
                        move |__aio_s: &mut Self, __aio_c: &::aio::Cx| __aio_s.#f(__aio_c, #key, #(#names),*)
                    )));
                }
            }
        }
    }

    /// A hidden function: `fun(&mut self, cx, key, vars…)` with `body`.
    fn hidden(&mut self, fun: &Ident, vars: &[Var], body: Tokens) {
        let (cx, r) = (&self.cx, &self.r);
        let params = vars.iter().map(|v| {
            let (p, t) = (&v.pat, v.ty.as_ref().expect("typed"));
            quote!(#p: #t)
        });
        self.out.fns.push(parse_quote! {
            #[doc(hidden)]
            #[allow(unused_mut, unused_variables, unreachable_code, clippy::too_many_arguments, clippy::needless_return)]
            fn #fun(&mut self, #cx: &::aio::Cx, __aio_key: ::std::string::String, #(#params),*)
                -> ::aio::Result<::aio::Step<Self, #r>> { #body }
        });
    }

    /// A join: the code after a branch or loop, run with what it uses
    /// (`rest` lowered by `joined`, after the branch, so awaits number in
    /// source order).
    fn join(&mut self, scope: &[Var], rest: &[Stmt], later: &Names, k: &Cont) -> syn::Result<Cont> {
        if rest.is_empty() {
            return Ok(k.clone());
        }
        let mut after = later.clone();
        rest.iter().for_each(|s| _ = names_of(quote!(#s), &mut after));
        self.joins += 1;
        Ok(Cont::Goto(format_ident!("__aio_{}_j{}", self.m, self.joins), live(scope, &after)?))
    }

    fn joined(&mut self, j: &Cont, k: &Cont, rest: &[Stmt], later: &Names, guard: &Names) -> syn::Result<()> {
        if let (Cont::Goto(fun, vars), false) = (j, rest.is_empty()) {
            let body = self.seq(rest, vars.clone(), later, k, guard)?;
            self.hidden(fun, vars, body);
        }
        Ok(())
    }

    fn await_of(&self, s: &Stmt, scope: &[Var], tail: bool, k: &Cont) -> syn::Result<Option<Await>> {
        let found = match s {
            Stmt::Local(l) => {
                let Some(init) = &l.init else { return Ok(None) };
                let Some((io, tried)) = awaited(&init.expr) else { return Ok(None) };
                if init.diverge.is_some() {
                    return err(l, "an awaited `let` can't have an `else`: match on the result after it");
                }
                let into = match &l.pat {
                    Pat::Wild(_) => Into::Drop,
                    Pat::Type(pt) if matches!(&*pt.pat, Pat::Wild(_)) => Into::Drop,
                    p => match let_vars(p).pop() {
                        Some(v) if v.ty.is_some() => Into::Bind(Box::new(v)),
                        _ => return err(p, "write `let x: T = io.await;`: T is how x is saved"),
                    },
                };
                Await { io, into, tried }
            }
            Stmt::Expr(Expr::Assign(a), Some(_)) => {
                let Some((io, tried)) = awaited(&a.right) else { return Ok(None) };
                let Expr::Path(p) = &*a.left else {
                    return err(&a.left, "an awaited result goes into one named value");
                };
                let Some(name) = p.path.get_ident() else {
                    return err(&a.left, "an awaited result goes into one named value");
                };
                let Some(v) = scope.iter().rev().find(|v| v.name == *name) else {
                    return err(name, &format!("`{name}` is not a typed `let` of this method"));
                };
                if v.ty.is_none() {
                    return err(
                        name,
                        &format!("give `{name}` a type (`let {name}: T = …;`): it is saved with the step"),
                    );
                }
                Await { io, into: Into::Bind(Box::new(v.clone())), tried }
            }
            Stmt::Expr(e, Some(_)) => match awaited(e) {
                Some((io, tried)) => Await { io, into: Into::Drop, tried },
                None => return Ok(None),
            },
            Stmt::Expr(e, None) if tail => match (awaited(e), k) {
                (Some((io, tried)), Cont::Return) => Await { io, into: Into::Answer, tried },
                (Some((io, tried)), Cont::Bind(_, _, r)) => Await { io, into: Into::Bind(r.clone()), tried },
                (Some(_), _) => return err(e, "an awaited value here goes nowhere: end it with `;`"),
                (None, _) => return Ok(None),
            },
            _ => return Ok(None),
        };
        if awaits(&found.io) {
            return err(&found.io, FORMS);
        }
        Ok(Some(found))
    }

    /// `stmts`, then `k`, in scope `scope`. `later`: the names used after
    /// them; `guard`: names a binding here must not shadow (a join or a
    /// loop's next turn reads the outer value under that name).
    fn seq(
        &mut self,
        stmts: &[Stmt],
        mut scope: Vec<Var>,
        later: &Names,
        k: &Cont,
        guard: &Names,
    ) -> syn::Result<Tokens> {
        let mut out = Tokens::new();
        for (i, s) in stmts.iter().enumerate() {
            let (rest, tail) = (&stmts[i + 1..], i + 1 == stmts.len());
            if let Some(a) = self.await_of(s, &scope, tail, k)? {
                if let Into::Bind(v) = &a.into
                    && guard.contains(&v.name.to_string())
                    && matches!(s, Stmt::Local(_))
                {
                    return err(&v.name, &shadows(&v.name));
                }
                out.extend(self.awaiting(a, &scope, rest, later, k, guard)?);
                return Ok(out);
            }
            if let Stmt::Local(l) = s
                && let Some(init) = &l.init
                && init.diverge.is_none()
                && matches!(&*init.expr, Expr::If(_) | Expr::Match(_) | Expr::Block(_))
                && awaits(&init.expr)
            {
                let Some(r) = let_vars(&l.pat).pop().filter(|v| v.ty.is_some()) else {
                    return err(&l.pat, "write `let x: T = …;`: T is how x is saved");
                };
                if guard.contains(&r.name.to_string()) {
                    return err(&r.name, &shadows(&r.name));
                }
                let mut after = later.clone();
                rest.iter().for_each(|s| _ = names_of(quote!(#s), &mut after));
                after.remove(&r.name.to_string());
                let vars: Vec<Var> = live(&scope, &after)?.into_iter().filter(|v| v.name != r.name).collect();
                self.joins += 1;
                let fun = format_ident!("__aio_{}_j{}", self.m, self.joins);
                let kb = Cont::Bind(fun, vars.clone(), Box::new(r));
                let mut g = guard.clone();
                g.extend(vars.iter().map(|v| v.name.to_string()));
                out.extend(self.construct(&init.expr, &scope, &after, &kb, &g)?);
                let (Cont::Goto(fun, params), false) = (kb.bound(), rest.is_empty()) else {
                    return Ok(out);
                };
                let body = self.seq(rest, params.clone(), later, k, guard)?;
                self.hidden(&fun, &params, body);
                return Ok(out);
            }
            let construct = match s {
                Stmt::Expr(e, _) if awaits(e) => Some(e),
                _ => None,
            };
            if let Some(e) = construct {
                let semi = matches!(s, Stmt::Expr(_, Some(_)));
                // The method's last expression: each branch ends with the answer.
                let kj = if tail && !semi { k.clone() } else { self.join(&scope, rest, later, k)? };
                let mut after = later.clone();
                rest.iter().for_each(|s| _ = names_of(quote!(#s), &mut after));
                let mut g = guard.clone();
                if let Cont::Goto(_, vars) = &kj {
                    g.extend(vars.iter().map(|v| v.name.to_string()));
                }
                out.extend(self.construct(e, &scope, &after, &kj, &g)?);
                self.joined(&kj, k, rest, later, guard)?;
                return Ok(out);
            }
            if awaits(s) {
                return err(s, FORMS);
            }
            let mut s = s.clone();
            Rewrite { lower: self, native: vec![], err: None }.stmt(&mut s)?;
            if let Stmt::Local(l) = &s {
                for v in let_vars(&l.pat) {
                    if guard.contains(&v.name.to_string()) {
                        return err(&v.name, &shadows(&v.name));
                    }
                    scope.push(v);
                }
            }
            match (&s, tail, k) {
                (Stmt::Expr(e, None), true, Cont::Return) => {
                    out.extend(quote!(return (#e).map(::aio::Step::Done);));
                    return Ok(out);
                }
                (Stmt::Expr(e, None), true, Cont::Goto(..)) => out.extend(quote!(#e;)),
                (Stmt::Expr(e, None), true, Cont::Bind(_, _, r)) => {
                    let (p, t) = (&r.pat, r.ty.as_ref().expect("typed"));
                    out.extend(quote!(let #p: #t = #e;));
                }
                _ => out.extend(quote!(#s)),
            }
        }
        out.extend(self.goto(k));
        Ok(out)
    }

    /// An await: its effect, started once this part commits, and the step
    /// it resumes in with the result.
    fn awaiting(
        &mut self,
        a: Await,
        scope: &[Var],
        rest: &[Stmt],
        later: &Names,
        k: &Cont,
        guard: &Names,
    ) -> syn::Result<Tokens> {
        let mut after = later.clone();
        rest.iter().for_each(|s| _ = names_of(quote!(#s), &mut after));
        self.awaits += 1;
        let n = self.awaits;
        let (step, fun) = (format!("{}.{n}", self.m), format_ident!("__aio_{}_{n}", self.m));
        // What the rest uses, then the result.
        let mut vars = live(scope, &after)?;
        let r_ty = self.r.clone();
        let result = match &a.into {
            Into::Bind(v) => {
                vars.retain(|x| x.name != v.name);
                Some((**v).clone())
            }
            Into::Drop => None,
            Into::Answer => {
                let name = format_ident!("__aio_answer");
                Some(Var { pat: parse_quote!(#name), name, ty: Some(r_ty), span: Span::call_site() })
            }
        };
        let mut params = vars.clone();
        params.extend(result.clone());
        let body = match &a.into {
            Into::Answer => quote!(return ::std::result::Result::Ok(::aio::Step::Done(__aio_answer));),
            _ => self.seq(rest, params.clone(), later, k, guard)?,
        };
        self.hidden(&fun, &params, body);
        let (names, tys) = (params.iter().map(|v| &v.name), params.iter().map(|v| v.ty.as_ref().expect("typed")));
        let (names2, cx, m) = (names.clone(), &self.cx, self.m.to_string());
        let ttl = match self.ttl {
            Some(ms) => quote!(::aio::ttl_key(cx, #m, #ms)),
            None => quote!(::std::option::Option::None),
        };
        self.out.arms.push(quote! {
            #step => {
                let (#(#names,)*): (#(#tys,)*) = ::aio::args(args)?;
                let __aio_key = match cx.by() {
                    ::aio::By::Effect(k) => k.clone(),
                    _ => ::std::string::String::from(#m),
                };
                let __aio_ttl = #ttl;
                let __aio_r = self.#fun(cx, __aio_key, #(#names2),*);
                ::aio::finish(self, cx, __aio_r, __aio_ttl)
            }
        });
        // In the effect: await, then hand the step what it needs.
        let tried = a.tried || matches!(a.into, Into::Answer);
        let recv = match (&result, tried) {
            (None, false) => quote!(let _ = __aio_io.await;),
            (None, true) => quote! {
                if let ::std::result::Result::Err(e) = ::std::result::Result::map_err(__aio_io.await, ::std::convert::Into::<::aio::Rejected>::into) {
                    return ::aio::fail(&__aio_h, #step, e, __aio_reply);
                }
            },
            (Some(v), false) => {
                let (name, ty) = (&v.name, v.ty.as_ref().expect("typed"));
                quote!(let #name: #ty = __aio_io.await;)
            }
            (Some(v), true) => {
                let (name, ty) = (&v.name, v.ty.as_ref().expect("typed"));
                quote! {
                    let #name: #ty = match ::std::result::Result::map_err(__aio_io.await, ::std::convert::Into::<::aio::Rejected>::into) {
                        ::std::result::Result::Ok(v) => v,
                        ::std::result::Result::Err(e) => return ::aio::fail(&__aio_h, #step, e, __aio_reply),
                    };
                }
            }
        };
        let names = params.iter().map(|v| &v.name);
        let ceiling = (!self.long).then(|| quote!(.timeout(::aio::effect::call_ceiling())));
        let mode = self.newest.then(|| quote!(.mode(::aio::Mode::Newest)));
        let io = &a.io;
        Ok(quote! {
            let __aio_io = #io;
            let __aio_reply = #cx.reply();
            #cx.run(::aio::Effect::new(&__aio_key, move |__aio_h: ::aio::Handle<Self>| async move {
                #recv
                ::aio::step(&__aio_h, #step, ::aio::json(&(#(#names,)*)), __aio_reply);
            })#mode #ceiling);
            return ::std::result::Result::Ok(::aio::Step::Wait);
        })
    }

    /// A list of statements in its own scope (a branch, an arm, a loop's body).
    fn branch(
        &mut self,
        stmts: &[Stmt],
        scope: Vec<Var>,
        later: &Names,
        k: &Cont,
        guard: &Names,
    ) -> syn::Result<Tokens> {
        let body = self.seq(stmts, scope, later, k, guard)?;
        Ok(quote!({ #body }))
    }

    fn bindings(&self, p: &Pat, scope: &[Var], guard: &Names) -> syn::Result<Vec<Var>> {
        let vs = untyped(p);
        if let Some(v) = vs.iter().find(|v| guard.contains(&v.name.to_string())) {
            return err(&v.name, &shadows(&v.name));
        }
        let mut s = scope.to_vec();
        s.extend(vs);
        Ok(s)
    }

    /// An `if`, `match`, `loop`, `while` or block with an await inside, then `k`.
    fn construct(&mut self, e: &Expr, scope: &[Var], after: &Names, k: &Cont, guard: &Names) -> syn::Result<Tokens> {
        match e {
            Expr::If(i) => {
                let (cond, then_scope) = match &*i.cond {
                    Expr::Let(l) => {
                        if awaits(&l.expr) {
                            return err(&l.expr, FORMS);
                        }
                        (&*i.cond, self.bindings(&l.pat, scope, guard)?)
                    }
                    c if awaits(c) => return err(c, FORMS),
                    c => (c, scope.to_vec()),
                };
                let then = self.branch(&i.then_branch.stmts, then_scope, after, k, guard)?;
                let other = match &i.else_branch {
                    None => {
                        let g = self.goto(k);
                        quote!({ #g })
                    }
                    Some((_, e)) => match &**e {
                        Expr::Block(b) => self.branch(&b.block.stmts, scope.to_vec(), after, k, guard)?,
                        e => {
                            let inner = self.construct(e, scope, after, k, guard)?;
                            quote!({ #inner })
                        }
                    },
                };
                Ok(quote!(if #cond #then else #other))
            }
            Expr::Match(m) => {
                if awaits(&m.expr) {
                    return err(&m.expr, FORMS);
                }
                let mut arms = vec![];
                for a in &m.arms {
                    if let Some((_, g)) = &a.guard
                        && awaits(g)
                    {
                        return err(g, FORMS);
                    }
                    let s = self.bindings(&a.pat, scope, guard)?;
                    let stmts = block_of(&a.body);
                    let body = self.branch(&stmts, s, after, k, guard)?;
                    let (pat, g) = (&a.pat, a.guard.as_ref().map(|(_, g)| quote!(if #g)));
                    arms.push(quote!(#pat #g => #body));
                }
                let scrutinee = &m.expr;
                Ok(quote!(match #scrutinee { #(#arms,)* }))
            }
            Expr::Block(b) if b.label.is_none() => self.branch(&b.block.stmts, scope.to_vec(), after, k, guard),
            Expr::Loop(l) => self.looped(l.label.as_ref(), None, &l.body, scope, after, k, guard),
            Expr::While(w) => {
                if awaits(&w.cond) {
                    return err(&w.cond, FORMS);
                }
                self.looped(w.label.as_ref(), Some(&w.cond), &w.body, scope, after, k, guard)
            }
            Expr::ForLoop(f) => err(
                &f.for_token,
                "an await inside `for` can't be resumed (its iterator isn't saved): keep the position in a typed \
                 `let` and loop with `while`",
            ),
            e => err(e, FORMS),
        }
    }

    /// A `loop` or `while` with an await inside: its turn is a join that
    /// runs again until a `break` (or the condition) goes on with `k`.
    #[allow(clippy::too_many_arguments)]
    fn looped(
        &mut self,
        label: Option<&Label>,
        cond: Option<&Expr>,
        body: &Block,
        scope: &[Var],
        after: &Names,
        k: &Cont,
        guard: &Names,
    ) -> syn::Result<Tokens> {
        let mut inside = after.clone();
        names_of(quote!(#body #cond), &mut inside);
        let vars = live(scope, &inside)?;
        self.joins += 1;
        let fun = format_ident!("__aio_{}_j{}", self.m, self.joins);
        let turn = Cont::Goto(fun.clone(), vars.clone());
        let mut g = guard.clone();
        g.extend(vars.iter().map(|v| v.name.to_string()));
        self.loops.push(Frame { label: label.map(|l| l.name.clone()), brk: k.clone(), next: turn.clone() });
        let (once, tail) = match cond {
            Some(Expr::Let(l)) => (self.bindings(&l.pat, &vars, &g)?, true),
            _ => (vars.clone(), cond.is_some()),
        };
        let lowered = self.branch(&body.stmts, once, &inside, &turn, &g);
        self.loops.pop();
        let lowered = lowered?;
        let turn_body = match (cond, tail) {
            (Some(c), true) => {
                let stop = self.goto(k);
                quote!(if #c #lowered else { #stop })
            }
            _ => lowered,
        };
        self.hidden(&fun, &vars, turn_body);
        Ok(self.goto(&turn))
    }
}

fn shadows(n: &Ident) -> String {
    format!("`{n}` hides a value of the same name that is used after this block, past an await: rename one of them")
}

fn block_of(e: &Expr) -> Vec<Stmt> {
    match e {
        Expr::Block(b) if b.label.is_none() && b.attrs.is_empty() => b.block.stmts.clone(),
        e => vec![Stmt::Expr(e.clone(), None)],
    }
}

/// In code that runs as it is: `return x` answers `x`; a `break` or
/// `continue` of a lowered loop goes on with its join.
struct Rewrite<'a> {
    lower: &'a Lower,
    /// The code's own loops and labeled blocks, innermost last: a label,
    /// and whether an unlabeled `break` stops there (a loop: yes; a block: no).
    native: Vec<(Option<Lifetime>, bool)>,
    err: Option<syn::Error>,
}

impl Rewrite<'_> {
    fn stmt(mut self, s: &mut Stmt) -> syn::Result<()> {
        self.visit_stmt_mut(s);
        self.err.map_or(Ok(()), Err)
    }

    /// The lowered loop a `break`/`continue` with this label leaves.
    fn target(&self, label: Option<&Lifetime>) -> Option<&Frame> {
        match label {
            None if self.native.iter().any(|(_, stops)| *stops) => None,
            None => self.lower.loops.last(),
            Some(l) if self.native.iter().any(|(n, _)| n.as_ref() == Some(l)) => None,
            Some(l) => self.lower.loops.iter().rev().find(|f| f.label.as_ref() == Some(l)),
        }
    }

    fn inside(&mut self, label: Option<&Label>, stops: bool, f: impl FnOnce(&mut Self)) {
        self.native.push((label.map(|l| l.name.clone()), stops));
        f(self);
        self.native.pop();
    }
}

impl VisitMut for Rewrite<'_> {
    fn visit_expr_mut(&mut self, e: &mut Expr) {
        match e {
            Expr::Closure(_) | Expr::Async(_) | Expr::Const(_) => return,
            Expr::Return(r) => {
                if let Some(v) = &mut r.expr {
                    self.visit_expr_mut(v);
                    *e = parse_quote!(return (#v).map(::aio::Step::Done));
                }
                return;
            }
            Expr::Break(b) => {
                if let Some(f) = self.target(b.label.as_ref()) {
                    if b.expr.is_some() {
                        self.err
                            .get_or_insert(syn::Error::new(b.span(), "a loop with an await can't break with a value"));
                    } else if matches!(f.brk, Cont::Return) {
                        self.err.get_or_insert(syn::Error::new(
                            b.span(),
                            "this loop ends the method: after a `break` it has no answer; put the answer after the loop",
                        ));
                    } else {
                        let g = self.lower.goto(&f.brk);
                        *e = parse_quote!({ #g });
                    }
                    return;
                }
            }
            Expr::Continue(c) => {
                if let Some(f) = self.target(c.label.as_ref()) {
                    let g = self.lower.goto(&f.next);
                    *e = parse_quote!({ #g });
                    return;
                }
            }
            Expr::Loop(l) => {
                let label = l.label.clone();
                return self.inside(label.as_ref(), true, |v| v.visit_block_mut(&mut l.body));
            }
            Expr::While(w) => {
                self.visit_expr_mut(&mut w.cond);
                let label = w.label.clone();
                return self.inside(label.as_ref(), true, |v| v.visit_block_mut(&mut w.body));
            }
            Expr::ForLoop(f) => {
                self.visit_expr_mut(&mut f.expr);
                let label = f.label.clone();
                return self.inside(label.as_ref(), true, |v| v.visit_block_mut(&mut f.body));
            }
            Expr::Block(b) if b.label.is_some() => {
                let label = b.label.clone();
                return self.inside(label.as_ref(), false, |v| v.visit_block_mut(&mut b.block));
            }
            _ => {}
        }
        syn::visit_mut::visit_expr_mut(self, e);
    }

    fn visit_item_mut(&mut self, _: &mut syn::Item) {}
}

/// Lower `pub async fn m(&mut self, cx: &Cx, …) -> Result<R>` (`f`, its
/// answer type `r`) into its first part (`m` itself, now a plain fn) and
/// hidden steps and joins, with their `apply` arms. `#[ttl(ms)]` keys its
/// effects by the arguments; `#[long]` lifts the call ceiling; `newest`:
/// `#[mode(newest)]`, a new call aborts the running one's await.
pub(crate) fn lower(f: &mut ImplItemFn, r: Type, ttl: Option<i64>, long: bool, newest: bool) -> syn::Result<Lowered> {
    let m = f.sig.ident.clone();
    let (mut cx, mut scope, mut first) = (None, vec![], vec![]);
    for (i, a) in f.sig.inputs.iter().skip(1).enumerate() {
        let FnArg::Typed(pt) = a else { continue };
        if i == 0 && crate::is_cx(&pt.ty) {
            let Pat::Ident(p) = &*pt.pat else { return err(pt, "name the `Cx` argument") };
            cx = Some(p.ident.clone());
            continue;
        }
        if crate::has_ref(&pt.ty) {
            return err(&pt.ty, "an async method takes its arguments owned: they live across awaits");
        }
        let Pat::Ident(p) = &*pt.pat else { return err(&pt.pat, "name each argument of an async method") };
        first.push(p.ident.clone());
        scope.push(Var { name: p.ident.clone(), pat: (*pt.pat).clone(), ty: Some((*pt.ty).clone()), span: pt.span() });
    }
    let cx = cx.unwrap_or_else(|| {
        f.sig.inputs.insert(1, parse_quote!(__aio_cx: &::aio::Cx));
        format_ident!("__aio_cx")
    });
    let mut l = Lower {
        m: m.clone(),
        cx: cx.clone(),
        r: r.clone(),
        ttl,
        long,
        newest,
        awaits: 0,
        joins: 0,
        loops: vec![],
        out: Lowered { fns: vec![], arms: vec![] },
    };
    let stmts = std::mem::take(&mut f.block.stmts);
    let body = l.seq(&stmts, scope, &Names::new(), &Cont::Return, &Names::new())?;
    if ttl.is_some() && l.awaits == 0 {
        return err(&f.sig, "#[ttl] caches what an async method awaits: this one awaits nothing");
    }
    let key = match ttl {
        Some(_) => {
            let name = m.to_string();
            quote!(let __aio_key = ::aio::effect::key(#name, &::aio::json(&(#(&#first,)*))?)?;)
        }
        None => {
            let name = m.to_string();
            quote!(let __aio_key = ::std::string::String::from(#name);)
        }
    };
    // A `let mut` may change only in a later step.
    f.attrs.push(parse_quote!(#[allow(unused_mut, unused_variables, unreachable_code, clippy::needless_return)]));
    f.sig.asyncness = None;
    f.sig.output = parse_quote!(-> ::aio::Result<::aio::Step<Self, #r>>);
    f.block = parse_quote!({ #key #body });
    Ok(l.out)
}

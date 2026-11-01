//! The Rust half of `ax lint`: what types and macros can't see. Cell
//! fields named like credentials, methods that aren't pure, hooks off the
//! top level of a component, and rsx! markup without accessible names.
use super::{Issue, Severity, secrets};
use dioxus_rsx::{AttributeName, AttributeValue, BodyNode, CallBody, ElementName, HotLiteral, IfChain};
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use syn::visit::{self, Visit};
use syn::{Attribute, Block, Expr, Item, ReturnType, Signature, spanned::Spanned};

pub struct Cell {
    pub id: String,
    pub name: String,
    pub file: String,
    pub line: usize,
}

/// What the files said, for the checks across files.
#[derive(Default)]
pub struct Facts {
    pub issues: Vec<Issue>,
    pub cells: Vec<Cell>,
    pub tests: usize,
    /// Every identifier test code names.
    pub tested: HashSet<String>,
}

fn last_is(path: &syn::Path, name: &str) -> bool {
    path.segments.last().is_some_and(|s| s.ident == name)
}

fn has(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|a| last_is(a.path(), name))
}

/// `#[test]`, `#[aio::test]`, `#[tokio::test]`.
fn is_test_fn(attrs: &[Attribute]) -> bool {
    has(attrs, "test")
}

/// `#[cfg(test)]`, `#[cfg(all(test, …))]`.
fn cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && a.meta.require_list().is_ok_and(|l| {
                fn any_test(ts: TokenStream) -> bool {
                    ts.into_iter().any(|t| match t {
                        TokenTree::Ident(i) => i == "test",
                        TokenTree::Group(g) => any_test(g.stream()),
                        _ => false,
                    })
                }
                any_test(l.tokens.clone())
            })
    })
}

fn test_item(item: &Item) -> bool {
    match item {
        Item::Fn(f) => is_test_fn(&f.attrs) || cfg_test(&f.attrs),
        Item::Mod(m) => cfg_test(&m.attrs),
        Item::Impl(i) => cfg_test(&i.attrs),
        Item::Use(u) => cfg_test(&u.attrs),
        _ => false,
    }
}

fn idents(ts: TokenStream, into: &mut HashSet<String>) {
    for t in ts {
        match t {
            TokenTree::Ident(i) => _ = into.insert(i.to_string()),
            TokenTree::Group(g) => idents(g.stream(), into),
            _ => {}
        }
    }
}

/// Test code out of `items` (what it names goes to `tested`); the rest stays.
fn split_tests(items: &mut Vec<Item>, f: &mut Facts) {
    items.retain(|item| {
        if !test_item(item) {
            return true;
        }
        idents(item.to_token_stream(), &mut f.tested);
        f.tests += count_tests(&item.to_token_stream());
        false
    });
    for item in items {
        if let Item::Mod(m) = item
            && let Some((_, inner)) = &mut m.content
        {
            split_tests(inner, f);
        }
    }
}

/// How many `#[…test]` attributes the tokens hold.
fn count_tests(ts: &TokenStream) -> usize {
    let tt: Vec<TokenTree> = ts.clone().into_iter().collect();
    tt.iter()
        .enumerate()
        .map(|(i, t)| match t {
            TokenTree::Group(g) if i > 0 && matches!(&tt[i - 1], TokenTree::Punct(p) if p.as_char() == '#') => {
                let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                usize::from(matches!(inner.last(), Some(TokenTree::Ident(id)) if id == "test"))
                    + count_tests(&g.stream())
            }
            TokenTree::Group(g) => count_tests(&g.stream()),
            _ => 0,
        })
        .sum()
}

/// One `.rs` file. A test file (under `tests/`) is no app code: only what
/// it tests is noted.
pub fn file(rel: &str, text: &str, test_file: bool, f: &mut Facts) {
    let mut ast = match syn::parse_file(text) {
        Ok(a) => a,
        Err(e) => {
            let line = e.span().start().line;
            f.issues.push(
                Issue::new(Severity::Warn, "scan", format!("not checked: it doesn't parse ({e})"), rel, line)
                    .fix("cargo check says more"),
            );
            return;
        }
    };
    renderer(rel, ast.to_token_stream(), &mut f.issues);
    if test_file {
        let ts = ast.to_token_stream();
        f.tests += count_tests(&ts);
        idents(ts, &mut f.tested);
        return;
    }
    split_tests(&mut ast.items, f);
    secrets::tokens(rel, ast.to_token_stream(), &mut f.issues);
    Checks { file: rel, f, repeats: 0 }.visit_file(&ast);
}

/// What an app names of the renderer behind `aio::ui` (a `dioxus…` path,
/// or a page script's `dioxus.send`): it ties the app to Dioxus, which air
/// replaces (docs/design/air.md); `aio::ui` stays.
fn renderer(rel: &str, ts: TokenStream, out: &mut Vec<Issue>) {
    fn walk(ts: TokenStream, lines: &mut BTreeSet<(usize, bool)>) {
        let tt: Vec<TokenTree> = ts.into_iter().collect();
        for (i, t) in tt.iter().enumerate() {
            match t {
                TokenTree::Group(g) => walk(g.stream(), lines),
                TokenTree::Ident(id) => {
                    let s = id.to_string();
                    let path = matches!(tt.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == ':');
                    let named = s == "dioxus" || s.starts_with("dioxus_");
                    let used = path
                        || matches!(tt.get(i.wrapping_sub(1)), Some(TokenTree::Ident(u)) if u == "use" || u == "crate");
                    if named && used {
                        lines.insert((id.span().start().line, false));
                    }
                }
                TokenTree::Literal(l) if l.to_string().contains("dioxus.") => {
                    lines.insert((l.span().start().line, true));
                }
                _ => {}
            }
        }
    }
    let mut lines = BTreeSet::new();
    walk(ts, &mut lines);
    for (line, script) in lines {
        let (msg, fix) = match script {
            false => (
                "names dioxus, the renderer behind aio::ui: the swap to air (docs/design/air.md) breaks it",
                "use aio::ui::prelude::*; and its names (View, signal, memo, effect, once, eval, …)",
            ),
            true => (
                "a page script talks to `dioxus`: its page object is `aio` (aio::ui::eval)",
                "aio.send(v) / await aio.recv()",
            ),
        };
        out.push(Issue::new(Severity::Error, "renderer", msg.into(), rel, line).fix(fix));
    }
}

/// A manifest's dependency on a renderer crate (`dioxus`, `dioxus-…`):
/// aio brings the one behind `aio::ui`.
pub fn manifest(rel: &str, text: &str, out: &mut Vec<Issue>) {
    for (i, l) in text.lines().enumerate() {
        let l = l.trim_start();
        let key = l.split(['=', '.', ' ', ']']).next().unwrap_or_default().trim_start_matches('[');
        let dep = key == "dioxus" || key.starts_with("dioxus-") || l.contains("dependencies.dioxus");
        if dep && !l.starts_with('#') {
            out.push(
                Issue::new(
                    Severity::Error,
                    "renderer",
                    "depends on dioxus, the renderer behind aio::ui".into(),
                    rel,
                    i + 1,
                )
                .fix("drop it: aio (feature kit) brings the views' renderer, named through aio::ui"),
            );
        }
    }
}

/// The id `#[cell]` gives `Name` without `id = "…"`.
pub fn snake(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

struct Checks<'a> {
    file: &'a str,
    f: &'a mut Facts,
    /// Closures and loops around here: an `rsx!` in one may make a list's items.
    repeats: usize,
}

impl Checks<'_> {
    fn push(&mut self, sev: Severity, area: &'static str, msg: String, line: usize, fix: &str) {
        self.f.issues.push(Issue::new(sev, area, msg, self.file, line).fix(fix));
    }

    /// A view (returns `View`, or Dioxus's `Element`) or a hook (`use_…`):
    /// what keeps something for it ([`HOOKS`]) runs on every render, in one order.
    fn hooks(&mut self, sig: &Signature, body: &Block) {
        let element = matches!(&sig.output, ReturnType::Type(_, t)
            if matches!(&**t, syn::Type::Path(p) if last_is(&p.path, "View") || last_is(&p.path, "Element")));
        if !element && !sig.ident.to_string().starts_with("use_") {
            return;
        }
        let mut h = Hooks::default();
        h.visit_block(body);
        for (hook, line) in h.found {
            self.push(
                Severity::Error,
                "hooks",
                format!("`{hook}` in `{}` runs only sometimes (in a branch, a loop or a closure, or after an early `return` or `?`): hooks are matched by call order, so a render that skips one hands the next hook its state", sig.ident),
                line,
                "call it at the top of the function, every render, before any early `return` or `?`, and branch on its value",
            );
        }
        for (made, line) in h.fresh {
            self.push(
                Severity::Error,
                "hooks",
                format!("`{made}` in the body of `{}` makes a new signal every render: its value is lost on the next one, and each copy is kept until the component goes", sig.ident),
                line,
                "make it once: `signal(|| ..)` (or `once(|| Signal::new(..))`)",
            );
        }
    }
}

impl Checks<'_> {
    /// A guard (`X.read()`, `.peek()`, `.write()`) taken in an `if let`,
    /// `while let` or `match` scrutinee lives until its block ends: a write
    /// in there, by a call or a method, panics as already borrowed.
    fn held(&mut self, scrutinee: &Expr, kind: &str, body: impl FnOnce(&mut Writes)) {
        let mut g = Guards::default();
        g.visit_expr(scrutinee);
        let Some((guard, line)) = g.0 else { return };
        let mut w = Writes::default();
        body(&mut w);
        let Some(write) = w.0 else { return };
        self.push(
            Severity::Warn,
            "render",
            format!("`{guard}` in this `{kind}` stays borrowed until its block ends, and the block writes (`{write}`): a write to the same signal panics as already borrowed"),
            line,
            "take the value first: `let v = X.get_untracked();` then `if let Some(id) = v { .. }` (or read with X.get())",
        );
    }
}

/// The scrutinees of the `let`s in a condition (`if let`, `let` chains).
#[derive(Default)]
struct Lets<'ast>(Vec<&'ast Expr>);

impl<'ast> Visit<'ast> for Lets<'ast> {
    fn visit_expr_let(&mut self, e: &'ast syn::ExprLet) {
        self.0.push(&e.expr);
    }
}

/// The first guard an expression takes and keeps: `.read()`, `.peek()`,
/// `.write()` with no argument, outside a closure.
#[derive(Default)]
struct Guards(Option<(String, usize)>);

impl<'ast> Visit<'ast> for Guards {
    fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
    fn visit_expr_method_call(&mut self, e: &'ast syn::ExprMethodCall) {
        let m = e.method.to_string();
        if self.0.is_none() && e.args.is_empty() && ["read", "read_untracked", "peek", "write"].contains(&m.as_str()) {
            let on = e.receiver.to_token_stream().to_string().replace(' ', "");
            self.0 = Some((format!("{on}.{m}()"), e.method.span().start().line));
        }
        visit::visit_expr_method_call(self, e);
    }
}

/// The first write a block may make: a signal's own (`.write()`, `.set(..)`,
/// `.with_mut(..)`…), or a call to a function (where one hides). A
/// constructor (`Some(..)`), a macro and a closure's body are none.
#[derive(Default)]
struct Writes(Option<String>);

impl<'ast> Visit<'ast> for Writes {
    fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
    fn visit_expr_method_call(&mut self, e: &'ast syn::ExprMethodCall) {
        let m = e.method.to_string();
        if self.0.is_none() && ["write", "set", "update", "with_mut", "toggle", "take", "replace"].contains(&m.as_str())
        {
            self.0 = Some(format!(".{m}()"));
        }
        visit::visit_expr_method_call(self, e);
    }
    fn visit_expr_call(&mut self, e: &'ast syn::ExprCall) {
        if let Expr::Path(p) = &*e.func
            && let Some(last) = p.path.segments.last()
            && self.0.is_none()
            && last.ident.to_string().starts_with(|c: char| c.is_lowercase())
        {
            self.0 = Some(format!("{}()", p.path.to_token_stream().to_string().replace(' ', "")));
        }
        visit::visit_expr_call(self, e);
    }
}

impl<'ast> Visit<'ast> for Checks<'_> {
    fn visit_item_struct(&mut self, s: &'ast syn::ItemStruct) {
        let Some(attr) = s.attrs.iter().find(|a| last_is(a.path(), "cell")) else { return };
        let (mut id, mut visible) = (None, false);
        if matches!(attr.meta, syn::Meta::List(_)) {
            _ = attr.parse_nested_meta(|m| {
                if m.path.is_ident("id") {
                    id = Some(m.value()?.parse::<syn::LitStr>()?.value());
                } else if m.input.peek(syn::Token![=]) {
                    visible |= m.path.is_ident("visible");
                    m.value()?.parse::<Expr>()?;
                }
                Ok(())
            });
        }
        let name = s.ident.to_string();
        let line = s.ident.span().start().line;
        self.f.cells.push(Cell {
            id: id.unwrap_or_else(|| snake(&name)),
            name: name.clone(),
            file: self.file.into(),
            line,
        });
        if visible {
            return; // `visible = f` decides what each viewer gets.
        }
        for field in &s.fields {
            let Some(ident) = &field.ident else { continue };
            let field_name = ident.to_string();
            if has(&field.attrs, "server") || !secrets::credential(&field_name) {
                continue;
            }
            self.push(
                Severity::Error,
                "security",
                format!("cell `{name}` field `{field_name}` is named like a credential, and every client of the cell gets it"),
                ident.span().start().line,
                "mark it #[server] (kept and saved, never sent) or filter it with #[cell(visible = f)]; if it really is public, say why: // aio-ok(security): <why>",
            );
        }
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        if has(&i.attrs, "methods") {
            let cell = i.self_ty.to_token_stream().to_string();
            for item in &i.items {
                let syn::ImplItem::Fn(m) = item else { continue };
                let mut p = Impure::default();
                p.visit_block(&m.block);
                for (path, what, line) in p.found {
                    self.push(
                        Severity::Error,
                        "cells",
                        format!("`{cell}::{}` uses {what} (`{path}`): methods replay from the log, so they must give the same result every time", m.sig.ident),
                        line,
                        fix_for(what),
                    );
                }
            }
        }
        visit::visit_item_impl(self, i);
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.hooks(&i.sig, &i.block);
        visit::visit_item_fn(self, i);
    }

    fn visit_expr_call(&mut self, e: &'ast syn::ExprCall) {
        // A hook's first value is made once: page CSS kept in one never sees
        // an edit, and live reload looks broken (`ax dev`).
        if let Expr::Path(p) = &*e.func
            && (last_is(&p.path, "once") || last_is(&p.path, "use_hook"))
            && e.args.to_token_stream().to_string().contains("page_css")
        {
            self.push(
                Severity::Error,
                "render",
                "page CSS made in `once` is made once: an edit to it never shows while `ax dev` runs".into(),
                p.span().start().line,
                "style { {aio::use_css!(dark; \"assets/style.css\")} } (live from the file, built in for release), or call aio::ui::page_css(..) in the component body",
            );
        }
        visit::visit_expr_call(self, e);
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        self.hooks(&i.sig, &i.block);
        visit::visit_impl_item_fn(self, i);
    }

    fn visit_expr_if(&mut self, e: &'ast syn::ExprIf) {
        let mut lets = Lets::default();
        lets.visit_expr(&e.cond);
        for scrutinee in lets.0 {
            self.held(scrutinee, "if let", |w| w.visit_block(&e.then_branch));
        }
        visit::visit_expr_if(self, e);
    }

    fn visit_expr_while(&mut self, e: &'ast syn::ExprWhile) {
        let mut lets = Lets::default();
        lets.visit_expr(&e.cond);
        for scrutinee in lets.0 {
            self.held(scrutinee, "while let", |w| w.visit_block(&e.body));
        }
        visit::visit_expr_while(self, e);
    }

    fn visit_expr_match(&mut self, e: &'ast syn::ExprMatch) {
        self.held(&e.expr, "match", |w| e.arms.iter().for_each(|a| w.visit_arm(a)));
        visit::visit_expr_match(self, e);
    }

    fn visit_expr_closure(&mut self, e: &'ast syn::ExprClosure) {
        self.repeats += 1;
        visit::visit_expr_closure(self, e);
        self.repeats -= 1;
    }

    fn visit_expr_for_loop(&mut self, e: &'ast syn::ExprForLoop) {
        self.repeats += 1;
        visit::visit_expr_for_loop(self, e);
        self.repeats -= 1;
    }

    fn visit_expr_loop(&mut self, e: &'ast syn::ExprLoop) {
        self.repeats += 1;
        visit::visit_expr_loop(self, e);
        self.repeats -= 1;
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        if last_is(&m.path, "rsx") {
            // One made once (a component's own) is in no list.
            let roots = if self.repeats == 0 { Nodes::Branch } else { Nodes::Macro };
            self.rsx(m.tokens.clone(), roots);
        }
        self.nested_rsx(m.tokens.clone());
    }
}

fn fix_for(what: &str) -> &'static str {
    match what {
        "the clock" => "use cx.now(): the time of the call, logged with it",
        "randomness" => "use cx.rand() or cx.id(): drawn once, logged with the call",
        "the environment" => "read it outside and pass it in as an argument",
        _ => {
            "do it in an effect (cx.run(Effect::new(…))): it runs after the commit and hands its result back as a call"
        }
    }
}

/// What a path in a method reaches that a replay can't repeat.
pub fn impure(segs: &[String]) -> Option<&'static str> {
    let s: Vec<&str> = segs.iter().map(String::as_str).collect();
    let after = |name: &str| s.iter().position(|x| *x == name).is_some_and(|i| i + 1 < s.len());
    let first = s.first().copied().unwrap_or("");
    let last = s.last().copied().unwrap_or("");
    Some(match s.as_slice() {
        [.., "SystemTime" | "Instant" | "Utc" | "Local" | "OffsetDateTime", "now" | "now_utc" | "now_local"] => {
            "the clock"
        }
        [.., "Uuid", "new_v4" | "new_v7" | "now_v7" | "now_v1"] | [.., "uuid_v4" | "uuid_v7" | "random_bytes"] => {
            "randomness"
        }
        _ if first == "rand" || ["thread_rng", "OsRng", "getrandom"].contains(&last) => "randomness",
        [.., "thread", "spawn" | "sleep" | "scope"] => "a thread",
        [r, .., "spawn" | "spawn_blocking" | "sleep"] if ["tokio", "async_std", "smol"].contains(r) => "a task",
        [.., "env", "var" | "var_os" | "vars" | "args" | "current_dir" | "set_var" | "remove_var"] => "the environment",
        [.., "io", "stdin" | "stdout" | "stderr"] => "terminal I/O",
        _ if after("fs") || after("File") || after("OpenOptions") => "file I/O",
        _ if ["TcpStream", "TcpListener", "UdpSocket", "Command"].iter().any(|n| after(n))
            || ["ureq", "reqwest", "hyper"].contains(&first) =>
        {
            "network or process I/O"
        }
        _ => return None,
    })
}

/// Paths in a method body that aren't pure. Closures and async blocks are
/// left out: they are effects, run after the commit.
#[derive(Default)]
struct Impure {
    found: Vec<(String, &'static str, usize)>,
}

impl<'ast> Visit<'ast> for Impure {
    fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
    fn visit_expr_async(&mut self, _: &'ast syn::ExprAsync) {}
    fn visit_item(&mut self, _: &'ast Item) {}
    fn visit_expr_path(&mut self, p: &'ast syn::ExprPath) {
        let segs: Vec<String> = p.path.segments.iter().map(|s| s.ident.to_string()).collect();
        if let Some(what) = impure(&segs) {
            self.found.push((segs.join("::"), what, p.span().start().line));
        }
    }
}

/// What keeps something for a view in aio::ui (beside the `use_…` hooks):
/// matched by call order, as Dioxus's hooks are.
const HOOKS: &[&str] =
    &["signal", "memo", "effect", "watch", "once", "on_cleanup", "callback", "provide", "task", "resource"];

/// Hook calls (`use_…()`, [`HOOKS`]) under a branch, a loop or a closure.
#[derive(Default)]
struct Hooks {
    depth: u32,
    /// Inside a closure or an async block: a `return` or `?` there leaves
    /// only it.
    inner: u32,
    /// A `return` or `?` came before: the hooks after it run only sometimes.
    exits: bool,
    found: Vec<(String, usize)>,
    /// `Signal::new(..)` in the body (not in a hook's closure): a new signal
    /// every render.
    fresh: Vec<(String, usize)>,
}

impl Hooks {
    fn nested(&mut self, f: impl FnOnce(&mut Self)) {
        self.depth += 1;
        f(self);
        self.depth -= 1;
    }
}

impl<'ast> Visit<'ast> for Hooks {
    fn visit_item(&mut self, _: &'ast Item) {}
    fn visit_expr_if(&mut self, e: &'ast syn::ExprIf) {
        self.visit_expr(&e.cond);
        self.nested(|h| {
            h.visit_block(&e.then_branch);
            if let Some((_, el)) = &e.else_branch {
                h.visit_expr(el);
            }
        });
    }
    fn visit_expr_match(&mut self, e: &'ast syn::ExprMatch) {
        self.visit_expr(&e.expr);
        self.nested(|h| e.arms.iter().for_each(|a| h.visit_arm(a)));
    }
    fn visit_expr_for_loop(&mut self, e: &'ast syn::ExprForLoop) {
        self.visit_expr(&e.expr);
        self.nested(|h| h.visit_block(&e.body));
    }
    fn visit_expr_while(&mut self, e: &'ast syn::ExprWhile) {
        self.nested(|h| visit::visit_expr_while(h, e));
    }
    fn visit_expr_loop(&mut self, e: &'ast syn::ExprLoop) {
        self.nested(|h| visit::visit_expr_loop(h, e));
    }
    fn visit_expr_closure(&mut self, e: &'ast syn::ExprClosure) {
        self.inner += 1;
        self.nested(|h| visit::visit_expr_closure(h, e));
        self.inner -= 1;
    }
    fn visit_expr_async(&mut self, e: &'ast syn::ExprAsync) {
        self.inner += 1;
        self.nested(|h| visit::visit_expr_async(h, e));
        self.inner -= 1;
    }
    fn visit_expr_return(&mut self, e: &'ast syn::ExprReturn) {
        visit::visit_expr_return(self, e);
        self.exits |= self.inner == 0;
    }
    fn visit_expr_try(&mut self, e: &'ast syn::ExprTry) {
        visit::visit_expr_try(self, e);
        self.exits |= self.inner == 0;
    }
    fn visit_expr_binary(&mut self, e: &'ast syn::ExprBinary) {
        if matches!(e.op, syn::BinOp::And(_) | syn::BinOp::Or(_)) {
            self.visit_expr(&e.left);
            self.nested(|h| h.visit_expr(&e.right));
        } else {
            visit::visit_expr_binary(self, e);
        }
    }
    fn visit_expr_call(&mut self, e: &'ast syn::ExprCall) {
        if let Expr::Path(p) = &*e.func
            && let Some(name) = p.path.segments.last().map(|s| s.ident.to_string())
            && (name.starts_with("use_") || HOOKS.contains(&name.as_str()))
            && (self.depth > 0 || self.exits)
        {
            self.found.push((name, p.span().start().line));
        }
        if let Expr::Path(p) = &*e.func
            && self.inner == 0
            && let [.., ty, f] = &p.path.segments.iter().collect::<Vec<_>>()[..]
            && ["Signal", "SyncSignal", "Memo"].contains(&ty.ident.to_string().as_str())
            && f.ident.to_string().starts_with("new")
        {
            self.fresh.push((format!("{}::{}", ty.ident, f.ident), p.span().start().line));
        }
        visit::visit_expr_call(self, e);
    }
}

// ── rsx! accessibility ──────────────────────────────────────────────────

impl Checks<'_> {
    /// `rsx!` calls inside other macros' tokens (and inside rsx! itself).
    fn nested_rsx(&mut self, ts: TokenStream) {
        let tt: Vec<TokenTree> = ts.into_iter().collect();
        for (i, t) in tt.iter().enumerate() {
            let TokenTree::Group(g) = t else { continue };
            if i >= 2
                && matches!(&tt[i - 1], TokenTree::Punct(p) if p.as_char() == '!')
                && matches!(&tt[i - 2], TokenTree::Ident(id) if id == "rsx")
            {
                self.rsx(g.stream(), Nodes::Macro);
            }
            self.nested_rsx(g.stream());
        }
    }

    fn rsx(&mut self, ts: TokenStream, roots: Nodes) {
        let Ok(body) = syn::parse2::<CallBody>(ts) else { return };
        let mut found = Vec::new();
        a11y(&body.body.roots, Within::default(), &mut found);
        keys(&body.body.roots, roots, &mut found);
        found.sort_by_key(|f| f.0);
        for (line, sev, area, msg, fix) in found {
            self.push(sev, area, msg, line, fix);
        }
    }
}

type Attrs = BTreeMap<String, Option<String>>;
/// Line, severity, area, what, the fix.
type Found = Vec<(usize, Severity, &'static str, String, &'static str)>;

/// An element's attributes, `aria-label` and `aria_label` alike; the value
/// when it is a literal.
fn attrs(el: &dioxus_rsx::Element) -> Option<Attrs> {
    let mut out = Attrs::new();
    for a in &el.raw_attributes {
        let name = match &a.name {
            AttributeName::BuiltIn(i) => i.to_string().trim_start_matches("r#").to_string(),
            AttributeName::Custom(s) => s.value(),
            AttributeName::Spread(_) => return None,
        };
        let value = match &a.value {
            AttributeValue::AttrLiteral(HotLiteral::Fmted(s)) => s.to_static(),
            AttributeValue::AttrLiteral(HotLiteral::Bool(b)) => Some(b.value.to_string()),
            AttributeValue::AttrLiteral(HotLiteral::Int(i)) => Some(i.base10_digits().to_string()),
            _ => None,
        };
        out.insert(name.replace('-', "_"), value);
    }
    el.spreads.is_empty().then_some(out)
}

/// Could the children give the element a name? Anything not known (an
/// expression, a component, a branch) counts as yes: a false alarm costs
/// more than a missed one.
fn speaks(nodes: &[BodyNode]) -> bool {
    nodes.iter().any(|n| match n {
        BodyNode::Text(t) => !t.input.source.value().trim().is_empty(),
        BodyNode::Element(el) => {
            attrs(el)
                .is_none_or(|a| ["aria_label", "aria_labelledby", "title", "alt"].iter().any(|k| a.contains_key(*k)))
                || speaks(&el.children)
        }
        _ => true,
    })
}

/// What `el` is missing to have an accessible name, and how to give it one.
fn unnamed(tag: &str, a: &Attrs, children: &[BodyNode], in_label: bool) -> Option<(String, &'static str)> {
    let key = |k: &str| a.contains_key(k);
    let val = |k: &str| a.get(k).cloned().flatten();
    let named = key("aria_label") || key("aria_labelledby") || key("title");
    if val("aria_hidden").as_deref() == Some("true") || matches!(val("role").as_deref(), Some("presentation" | "none"))
    {
        return None;
    }
    let (what, fix) = match tag {
        "img" if !key("alt") && !named => {
            ("has no alt text", "alt: \"what it shows\" (alt: \"\" when it is decoration)")
        }
        "button" if !named && !speaks(children) => ("has no accessible name", "give it text, aria_label or title"),
        "input" | "select" | "textarea" => {
            let kind = match tag {
                "input" if key("type") => val("type").map(|t| t.to_lowercase()),
                _ => Some("text".into()),
            };
            match kind.as_deref() {
                // Unknown (an expression), or named by the browser.
                None | Some("hidden" | "submit" | "reset") => return None,
                Some("image") if !key("alt") && !named => ("has no alt text", "alt: \"what it does\""),
                Some("button") if !key("value") && !named => ("has no accessible name", "value: \"what it does\""),
                Some("image" | "button") => return None,
                _ if named || in_label || key("id") || key("placeholder") => return None,
                _ => ("has no label", "aria_label: \"…\", or wrap it in label { }"),
            }
        }
        _ => return None,
    };
    Some((format!("<{tag}> {what}: screen readers and agents can't name it"), fix))
}

/// A click on an element the keyboard can't reach or a reader can't name.
/// Its keys may be elsewhere: an item of a composite (the list's arrows), a
/// dialog or notice (Escape), presentation, an owner pointing at its active item.
fn keyless(tag: &str, a: &Attrs) -> Option<String> {
    const NATIVE: [&str; 10] =
        ["a", "button", "input", "select", "textarea", "label", "summary", "option", "details", "dialog"];
    const KEYS_ELSEWHERE: [&str; 14] = [
        "option",
        "row",
        "gridcell",
        "treeitem",
        "tab",
        "menuitem",
        "menuitemcheckbox",
        "menuitemradio",
        "dialog",
        "alertdialog",
        "alert",
        "status",
        "presentation",
        "none",
    ];
    const OWNERS: [&str; 7] = ["listbox", "grid", "treegrid", "tree", "tablist", "menu", "menubar"];
    let role = a.get("role").map(|r| r.as_deref());
    let elsewhere = match role {
        Some(Some(r)) => {
            KEYS_ELSEWHERE.contains(&r) || (OWNERS.contains(&r) && a.contains_key("aria_activedescendant"))
        }
        // An expression: it may be any role.
        Some(None) => true,
        None => false,
    };
    let keys = ["onkeydown", "onkeyup", "onkeypress"].iter().any(|k| a.contains_key(*k));
    (a.contains_key("onclick") && !keys && !elsewhere && !NATIVE.contains(&tag) && !tag.contains('-'))
        .then(|| format!("<{tag}> takes clicks but no keys: keyboard users and agents can't use it"))
}

/// Markup that does what its author didn't mean (aio's dev warnings), with the fix.
fn misused(tag: &str, a: &Attrs, within: Within) -> Vec<(String, &'static str)> {
    let val = |k: &str| a.get(k).cloned().flatten();
    let on = |k: &str| matches!(val(k).as_deref(), Some("true"));
    let mut out = vec![];
    if tag == "button" && within.form && !a.contains_key("type") {
        let fix = "r#type: \"button\" (or \"submit\" for the form's own)";
        out.push(("<button> in a form without a type submits it".into(), fix));
    }
    if tag == "a" && a.contains_key("onclick") && !a.contains_key("href") {
        out.push(("<a> with onclick and no href can't be focused or announced".into(), "use a button for an action"));
    }
    if let Some(n) = val("tabindex").and_then(|t| t.trim().parse::<i64>().ok()).filter(|&n| n > 0) {
        let fix = "tabindex: 0 (in document order) or -1 (from code only)";
        out.push((format!("tabindex {n} reorders the whole page's tab sequence"), fix));
    }
    let focusable = ["a", "button", "input", "select", "textarea", "summary"].contains(&tag)
        || val("tabindex").and_then(|t| t.trim().parse::<i64>().ok()).is_some_and(|n| n >= 0);
    if on("aria_hidden") && focusable {
        let fix = "tabindex: -1 or inert as well";
        out.push((format!("<{tag}> is aria-hidden but focusable: the keyboard lands on what a reader hides"), fix));
    }
    if on("aria_disabled") && a.contains_key("onclick") && !a.contains_key("disabled") {
        let fix = "disabled: true, or return early in the handler";
        out.push((format!("<{tag}> is aria-disabled but its onclick still runs"), fix));
    }
    out
}

/// A list's items without a `key` (aio's missing-key tripwire): matched by
/// place, a row removed or moved hands its state (a field's text, focus,
/// a component's signals) to its neighbour. Duplicate keys Dioxus itself
/// refuses (`rsx!` and debug builds).
fn unkeyed(roots: &[BodyNode]) -> Found {
    // The body's first node keys each pass: only that one takes a key.
    let missing = match roots.first() {
        Some(BodyNode::Element(el)) if !has_key(el) => Some(el_name(&el.name)),
        Some(BodyNode::Component(c)) if c.get_key().is_none() => Some(component_name(c)),
        // A block's rsx! is a node inside the pass's unkeyed one: its key keys nothing.
        Some(BodyNode::RawExpr(e)) => {
            let msg = "a for loop over a block keys nothing (the block's rsx! sits inside an unkeyed node): a shift of the list rebuilds every row";
            return vec![(
                e.span().start().line,
                Severity::Warn,
                "render",
                msg.into(),
                "{items.iter().map(|it| { …; rsx! { div { key: \"{it.id}\", … } } })}",
            )];
        }
        _ => None,
    };
    let say = |(what, line)| {
        let msg = format!(
            "<{what}> in a for loop has no key: a removed or moved row hands its state to the next (air, aio::ui's next renderer, requires one)"
        );
        (line, Severity::Error, "render", msg, "key: \"{item.id}\"")
    };
    missing.map(say).into_iter().collect()
}

fn has_key(el: &dioxus_rsx::Element) -> bool {
    el.raw_attributes.iter().any(|a| a.name.is_likely_key())
}

/// An element's tag and line.
fn el_name(name: &ElementName) -> (String, usize) {
    match name {
        ElementName::Ident(i) => (i.to_string(), i.span().start().line),
        ElementName::Custom(s) => (s.value(), s.span().start().line),
    }
}

/// A component's name and line.
fn component_name(c: &dioxus_rsx::Component) -> (String, usize) {
    (c.name.segments.last().map_or(String::new(), |s| s.ident.to_string()), c.name.span().start().line)
}

/// What a run of nodes is, for the keys on them.
#[derive(Clone, Copy, PartialEq)]
enum Nodes {
    /// A `for` loop's body: each pass is an item of a list.
    Loop,
    /// An `rsx!` call's roots in a closure or a loop: an item when the
    /// call is one (`.map(|t| rsx! { … })`).
    Macro,
    /// A branch's or a component's children, or an `rsx!` made once: a
    /// block in no list.
    Branch,
    /// An element's children.
    Children,
}

/// Keys Dioxus ignores without a word at run time: one on a block's later
/// node (the first node keys the block: "Keys are only allowed on the
/// first node in the block"), and one on a node in no list (only a list's
/// items are told apart by key: a changed key remounts nothing).
fn keys(nodes: &[BodyNode], block: Nodes, out: &mut Found) {
    for (i, n) in nodes.iter().enumerate() {
        let keyed = match n {
            BodyNode::Element(el) => has_key(el).then(|| el_name(&el.name)),
            BodyNode::Component(c) => c.get_key().is_some().then(|| component_name(c)),
            _ => None,
        };
        if let Some((what, line)) = keyed {
            if i > 0 && block != Nodes::Children {
                let msg = format!(
                    "key on <{what}>, not its block's first node: aio::ui ignores it (the first node keys the block)"
                );
                out.push((
                    line,
                    Severity::Warn,
                    "render",
                    msg,
                    "Fragment { key: \"{item.id}\", … } around the block's nodes",
                ));
            } else if !matches!(block, Nodes::Loop | Nodes::Macro) {
                let msg = format!("key on <{what}> in no list: aio::ui never remounts it when the key changes");
                out.push((line, Severity::Warn, "render", msg, "a one-item for: for k in [key] { … key: \"{k}\" … }"));
            }
        }
        match n {
            BodyNode::Element(el) => keys(&el.children, Nodes::Children, out),
            BodyNode::Component(c) => keys(&c.children.roots, Nodes::Branch, out),
            BodyNode::ForLoop(f) => keys(&f.body.roots, Nodes::Loop, out),
            BodyNode::IfChain(c) => {
                let mut at = Some(c);
                while let Some(c) = at {
                    if let Expr::Let(l) = &*c.cond {
                        let msg = "`if let` in rsx!: air (aio::ui's next renderer) refuses it".to_string();
                        let fix = "branch on a bool (`if x.is_some()`), or a child: {x.map(|v| rsx! { … })}";
                        out.push((l.let_token.span.start().line, Severity::Warn, "render", msg, fix));
                    }
                    keys(&c.then_branch.roots, Nodes::Branch, out);
                    if let Some(e) = &c.else_branch {
                        keys(&e.roots, Nodes::Branch, out);
                    }
                    at = c.else_if_branch.as_deref();
                }
            }
            _ => {}
        }
    }
}

/// Where an element sits: in a `label`, in a `form`.
#[derive(Clone, Copy, Default)]
struct Within {
    label: bool,
    form: bool,
}

fn a11y(nodes: &[BodyNode], within: Within, out: &mut Found) {
    for n in nodes {
        match n {
            BodyNode::Element(el) => {
                let (tag, line) = el_name(&el.name);
                if let Some(a) = attrs(el) {
                    out.extend(
                        unnamed(&tag, &a, &el.children, within.label)
                            .map(|(msg, fix)| (line, Severity::Error, "a11y", msg, fix)),
                    );
                    out.extend(keyless(&tag, &a).map(|msg| {
                        (line, Severity::Warn, "a11y", msg, "use a button, or give it role, tabindex: 0 and onkeydown")
                    }));
                    out.extend(
                        misused(&tag, &a, within)
                            .into_iter()
                            .map(|(msg, fix)| (line, Severity::Warn, "a11y", msg, fix)),
                    );
                }
                if drag_set(el) {
                    let msg = format!(
                        "<{tag}>'s ondragstart set_data never reaches the drop: the view runs after the browser's dragstart"
                    );
                    out.push((
                        line,
                        Severity::Warn,
                        "render",
                        msg,
                        "\"data-drag\": \"{id}\" on the element (the drop reads it as text/plain)",
                    ));
                }
                let within = Within { label: within.label || tag == "label", form: within.form || tag == "form" };
                a11y(&el.children, within, out);
            }
            BodyNode::Component(c) => a11y(&c.children.roots, within, out),
            BodyNode::ForLoop(f) => {
                out.extend(unkeyed(&f.body.roots));
                a11y(&f.body.roots, within, out)
            }
            BodyNode::IfChain(c) => chain(c, within, out),
            _ => {}
        }
    }
}

/// An `ondragstart` that sets the drag's data: lost, since a view's handler
/// runs after the browser's dragstart is over (aio's page sets `data-drag`).
fn drag_set(el: &dioxus_rsx::Element) -> bool {
    el.raw_attributes.iter().any(|a| {
        matches!(&a.name, AttributeName::BuiltIn(i) if i == "ondragstart")
            && a.value.to_token_stream().to_string().contains("set_data")
    })
}

fn chain(c: &IfChain, within: Within, out: &mut Found) {
    a11y(&c.then_branch.roots, within, out);
    if let Some(e) = &c.else_if_branch {
        chain(e, within, out);
    }
    if let Some(e) = &c.else_branch {
        a11y(&e.roots, within, out);
    }
}

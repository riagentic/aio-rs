//! air's `rsx!` and `#[component]`. Dioxus's syntax, a different output
//! (its paths start at `air`: the crate, or the name `aio::ui::prelude`
//! brings in, so an app needs no `air` dependency of its own):
//! one static template per element tree (its markup pre-encoded as a byte
//! string: one token, whatever its size) and one call per dynamic hole.
//! No syn: the input is split on token trees, expressions are passed
//! through as written (their spans kept), so the macro runs in
//! microseconds and its expansion stays near the size of its input.
use proc_macro2::{Delimiter, Ident, Literal, Spacing, Span, TokenStream, TokenTree};
use quote::{format_ident, quote, quote_spanned};

/// Elements, text, `{expr}`, components, `for … in … { … }` (keyed rows:
/// `key:` on the row), `if … { … } else { … }`, as Dioxus's `rsx!`. A
/// literal is static; a `"{x}"` string and every other expression is a
/// hole that follows what it reads (the names it uses are cloned in).
#[proc_macro]
pub fn rsx(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let toks: Vec<TokenTree> = TokenStream::from(input).into_iter().collect();
    match parse_nodes(&toks) {
        Ok(nodes) => body(&nodes).into(),
        Err((span, msg)) => quote_spanned!(span=> ::core::compile_error!(#msg)).into(),
    }
}

/// `fn Row(name: String, on: Signal<bool>) -> View`: a props struct
/// `RowProps { pub name, pub on }` and `fn Row(RowProps { name, on }: RowProps)`,
/// called as `Row { name: …, on }` in `rsx!`. No arguments: a plain `fn() -> View`.
#[proc_macro_attribute]
pub fn component(_: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let toks: Vec<TokenTree> = TokenStream::from(item).into_iter().collect();
    let Some(fn_at) = toks.iter().position(|t| is_ident(t, "fn")) else {
        return quote!(::core::compile_error!("air: #[component] goes on a fn")).into();
    };
    let (name, args) = match (toks.get(fn_at + 1), toks.get(fn_at + 2)) {
        (Some(TokenTree::Ident(n)), Some(TokenTree::Group(g))) if g.delimiter() == Delimiter::Parenthesis => {
            (n.clone(), g.stream())
        }
        _ => {
            return quote!(::core::compile_error!(
                "air: #[component] goes on a plain `fn Name(props…) -> View` (no generics)"
            ))
            .into();
        }
    };
    let head: TokenStream = toks[..fn_at].iter().cloned().collect();
    let rest: TokenStream = toks[fn_at + 3..].iter().cloned().collect();
    let args: Vec<Vec<TokenTree>> = split_top(&args.into_iter().collect::<Vec<_>>());
    if args.is_empty() {
        return quote!(#[allow(non_snake_case)] #head fn #name() #rest).into();
    }
    let props = format_ident!("{}Props", name);
    let (mut fields, mut pats) = (vec![], vec![]);
    for a in &args {
        // `#[props(default)]` and other attributes on a prop: Dioxus's; air's props are all given.
        let a: Vec<TokenTree> = strip_attrs(a);
        let a = &a;
        let colon = a.iter().position(|t| matches!(t, TokenTree::Punct(p) if p.as_char() == ':')).unwrap_or(0);
        let pat: TokenStream = a[..colon].iter().cloned().collect();
        let ty: TokenStream = a[colon + 1..].iter().cloned().collect();
        let field = a[..colon].iter().rev().find_map(|t| match t {
            TokenTree::Ident(i) => Some(i.clone()),
            _ => None,
        });
        fields.push(quote!(pub #field: #ty));
        pats.push(pat);
    }
    let vis = toks[..fn_at].iter().find(|t| is_ident(t, "pub")).map(|_| quote!(pub));
    quote!(
        #[derive(Clone)]
        #vis struct #props { #(#fields),* }
        #[allow(non_snake_case)]
        #head fn #name(#props { #(#pats),* }: #props) #rest
    )
    .into()
}

/// Props structs need nothing derived on air (`#[derive(Props)]` is
/// kept so the facade's code builds); `#[props(…)]` is accepted.
#[proc_macro_derive(Props, attributes(props))]
pub fn props(_: proc_macro::TokenStream) -> proc_macro::TokenStream {
    proc_macro::TokenStream::new()
}

/// `toks` without its `#[…]` attributes.
fn strip_attrs(toks: &[TokenTree]) -> Vec<TokenTree> {
    let mut out = vec![];
    let mut i = 0;
    while i < toks.len() {
        if is_punct(toks.get(i), '#')
            && matches!(toks.get(i + 1), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket)
        {
            i += 2;
            continue;
        }
        out.push(toks[i].clone());
        i += 1;
    }
    out
}

type Res<T> = Result<T, (Span, String)>;

enum Node {
    Text(String),
    /// A `"…{x}…"` text: `format!`'s literal and arguments, and the names it reads.
    Fmt(TokenStream, Vec<String>),
    Expr(TokenStream),
    El {
        tag: String,
        attrs: Vec<(String, Val)>,
        children: Vec<Node>,
        key: Option<TokenStream>,
    },
    Comp {
        path: Vec<TokenTree>,
        props: Vec<(Ident, TokenStream)>,
        children: Vec<Node>,
        key: Option<TokenStream>,
    },
    For {
        pat: TokenStream,
        expr: TokenStream,
        body: Vec<Node>,
    },
    If {
        cond: TokenStream,
        then: Vec<Node>,
        otherwise: Vec<Node>,
    },
}

enum Val {
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
        if toks.get(*i).is_some_and(|t| is_ident(t, "let")) {
            return Err((
                span_of(toks, *i),
                "air: `if let` in rsx!: make a bool (`x.is_some()`) or a {child} hole".into(),
            ));
        }
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
    if let TokenTree::Ident(_) = t {
        let start = *i;
        let (path, inner) = until_block(toks, i)?;
        let path: Vec<TokenTree> = path.into_iter().collect();
        let last = path.iter().rev().find_map(|t| match t {
            TokenTree::Ident(i) => Some(i.to_string()),
            _ => None,
        });
        let comp = path.len() > 1 || last.as_ref().is_some_and(|s| s.starts_with(|c: char| c.is_uppercase()));
        if path.is_empty() || path.iter().any(|t| !matches!(t, TokenTree::Ident(_) | TokenTree::Punct(_))) {
            return Err((span_of(toks, start), "air: expected an element or a component".into()));
        }
        return element(path, &inner, comp);
    }
    Err((t.span(), "air: unexpected in rsx!".into()))
}

/// An element's or component's `{ … }`: `name: value,` pairs, then children.
fn element(path: Vec<TokenTree>, toks: &[TokenTree], comp: bool) -> Res<Node> {
    let (mut attrs, mut props, mut children, mut key, mut i) = (vec![], vec![], vec![], None, 0);
    while i < toks.len() {
        if is_punct(toks.get(i), ',') {
            i += 1;
            continue;
        }
        if is_punct(toks.get(i), '.') && is_punct(toks.get(i + 1), '.') {
            return Err((
                toks[i].span(),
                "air: attribute spreads (`..attributes`) come with the kit's move to air (plan step 2)".into(),
            ));
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
            let TokenTree::Ident(id) = name_tok else { return Err((name_tok.span(), "air: a prop is a name".into())) };
            props.push((id, value));
        } else {
            if let Some(ev) = name.strip_prefix("on")
                && payload(ev).is_none()
            {
                return Err((
                    name_tok.span(),
                    format!("air: no event `{name}` (a DOM event's name, as in `onclick`, `onkeydown`, `oninput`)"),
                ));
            }
            attrs.push((attr_name(&name), attr_val(&name, value)));
        }
    }
    if comp {
        return Ok(Node::Comp { path, props, children, key });
    }
    let tag = path[0].to_string().trim_start_matches("r#").to_string();
    Ok(Node::El { tag, attrs, children, key })
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
/// handler gets (as Dioxus's event tables type them).
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
        "input" | "change" | "submit" | "reset" | "invalid" => "FormData",
        "scroll" | "scrollend" => "ScrollData",
        "mounted" => "",
        "focus" | "blur" | "focusin" | "focusout" | "copy" | "cut" | "paste" | "compositionstart"
        | "compositionupdate" | "compositionend" | "animationstart" | "animationend" | "animationiteration"
        | "transitionend" | "transitionstart" | "transitionrun" | "transitioncancel" | "toggle" | "beforetoggle"
        | "load" | "error" | "abort" | "canplay" | "canplaythrough" | "durationchange" | "emptied" | "encrypted"
        | "ended" | "loadeddata" | "loadedmetadata" | "loadstart" | "pause" | "play" | "playing" | "progress"
        | "ratechange" | "seeked" | "seeking" | "stalled" | "suspend" | "timeupdate" | "volumechange" | "waiting"
        | "select" | "selectstart" | "selectionchange" | "resize" | "visible" | "cancel" | "close" => "PlainData",
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
    let mut bound = vec![];
    scan(&toks, out, &mut bound);
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

fn scan(toks: &[TokenTree], out: &mut Vec<String>, bound: &mut Vec<String>) {
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
                idents_in(&toks[i + 1..end], bound);
                i = end + 1;
                continue;
            }
            TokenTree::Ident(id) if id == "let" => {
                let end = (i + 1..toks.len()).find(|k| is_punct(toks.get(*k), '=')).unwrap_or(toks.len());
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
                    || matches!(next, Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis);
                if is_punct(next, '!') {
                    // A macro: the `{name}`s of its format string, then its arguments.
                    if let Some(TokenTree::Group(g)) = toks.get(i + 2) {
                        let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                        // A view inside: the names its nodes read, not its tags.
                        if s == "rsx" {
                            if let Ok(nodes) = parse_nodes(&inner) {
                                let mut r = vec![];
                                node_captures(&nodes, &mut r);
                                out.extend(r.into_iter().filter(|n| !bound.contains(n)));
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
                        scan(&inner, out, bound);
                    }
                    i += 3;
                    continue;
                }
                if !skip && !out.contains(&s) {
                    out.push(s);
                }
            }
            TokenTree::Group(g) => scan(&g.stream().into_iter().collect::<Vec<_>>(), out, bound),
            _ => {}
        }
        i += 1;
    }
}

/// `{ let a = a.clone(); … }` for the names a hole reads.
fn clones(names: &[String]) -> TokenStream {
    let n = names.iter().map(|n| Ident::new(n, Span::call_site()));
    quote!(#(#[allow(unused_mut)] let mut #n = ::std::clone::Clone::clone(&#n);)*)
}

fn node_captures(nodes: &[Node], out: &mut Vec<String>) {
    for n in nodes {
        match n {
            Node::Text(_) => {}
            Node::Fmt(_, r) => out.extend(r.iter().cloned()),
            Node::Expr(e) => captures(e, out),
            Node::El { attrs, children, key, .. } => {
                for (_, v) in attrs {
                    match v {
                        Val::Fmt(_, r) => out.extend(r.iter().cloned()),
                        Val::Event(_, e) | Val::Expr(e) => captures(e, out),
                        _ => {}
                    }
                }
                key.iter().for_each(|k| captures(k, out));
                node_captures(children, out);
            }
            Node::Comp { props, children, key, .. } => {
                props.iter().for_each(|(_, e)| captures(e, out));
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
                node_captures(then, out);
                node_captures(otherwise, out);
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

/// Is `n` a placeholder in its template (an anchor its content goes before)?
fn anchored(n: &Node) -> bool {
    matches!(n, Node::Expr(_) | Node::Comp { .. } | Node::For { .. } | Node::If { .. })
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
            Node::El { tag, attrs, children, .. } => {
                self.bytes.push(1);
                self.str(tag);
                let stat: Vec<(&String, String)> = attrs
                    .iter()
                    .filter_map(|(k, v)| match v {
                        Val::Static(s) => Some((k, s.clone())),
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
                let dynamic = attrs.iter().any(|(_, v)| !matches!(v, Val::Static(_) | Val::Present(_)));
                self.bytes.push(dynamic as u8);
                if dynamic || root {
                    let id = self.mark();
                    for (k, v) in attrs {
                        let hole = match v {
                            Val::Fmt(f, r) => {
                                let c = clones(r);
                                quote!(air::__rt::attr(#id, #k, { #c ::std::boxed::Box::new(move || air::IntoAttr::into_attr(::std::format!(#f))) });)
                            }
                            Val::Expr(e) => {
                                hole_fn(e, quote!(air::IntoAttr::into_attr), |e| quote!(air::__rt::attr(#id, #k, #e);))
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
                                    let closure = e.clone().into_iter().take(2).any(|t| is_punct(Some(&t), '|'));
                                    let h = if closure { quote!(handler_fn) } else { quote!(handler) };
                                    quote!(air::__rt::on(#id, #dom, { #c air::__rt::#h::<air::__rt::#data>(#e) });)
                                }
                            }
                            _ => continue,
                        };
                        self.holes.push(hole);
                    }
                }
                for c in children {
                    self.node(c, false);
                }
            }
            n => {
                self.bytes.push(4);
                let id = self.mark();
                let hole = match n {
                    Node::Expr(e) => {
                        hole_fn(e, quote!(air::IntoChild::into_child), |e| quote!(air::__rt::child(#id, #e);))
                    }
                    Node::Comp { props, children, .. } => {
                        let mut r = vec![];
                        props.iter().for_each(|(_, e)| captures(e, &mut r));
                        node_captures(children, &mut r);
                        let c = clones(&r);
                        let call = comp(n);
                        quote!(air::__rt::comp(#id, { #c ::std::boxed::Box::new(move || { #c #call }) });)
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
                            { #ce move || ::std::iter::IntoIterator::into_iter(#expr).collect::<::std::vec::Vec<_>>() },
                            { #ck move |#pat: &_| (#key).to_owned() },
                            { #cr move |#pat| #view });)
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

/// A hole's closure: a single name is cloned per run (its type decides
/// whether it follows anything: a signal does), anything else runs as written.
fn hole_fn(e: &TokenStream, into: TokenStream, call: impl Fn(TokenStream) -> TokenStream) -> TokenStream {
    let v: Vec<TokenTree> = e.clone().into_iter().collect();
    if let [TokenTree::Ident(i)] = v.as_slice() {
        return call(
            quote!({ let #i = ::std::clone::Clone::clone(&#i); ::std::boxed::Box::new(move || #into(::std::clone::Clone::clone(&#i))) }),
        );
    }
    let mut r = vec![];
    captures(e, &mut r);
    let c = clones(&r);
    call(quote!({ #c ::std::boxed::Box::new(move || #into({ #e })) }))
}

/// `Row { a: x, children… }` → `Row(RowProps { a: x.into(), children: … })`; no props: `Row()`.
fn comp(n: &Node) -> TokenStream {
    let Node::Comp { path, props, children, .. } = n else { unreachable!() };
    let (head, last) = path.split_at(path.len() - 1);
    let head: TokenStream = head.iter().cloned().collect();
    let name = &last[0];
    if props.is_empty() && children.is_empty() {
        return quote!(#head #name());
    }
    let props_ty = format_ident!("{}Props", name.to_string());
    let fields = props.iter().map(|(k, v)| quote!(#k: ::std::convert::Into::into(#v)));
    let kids = (!children.is_empty()).then(|| {
        let v = body(children);
        quote!(children: #v,)
    });
    quote!(#head #name(#head #props_ty { #(#fields,)* #kids }))
}

/// The view of `nodes`: one template and its holes, built where it is
/// placed (the names it reads are cloned in first).
fn body(nodes: &[Node]) -> TokenStream {
    if nodes.is_empty() {
        return quote!(::core::result::Result::<air::Tree, air::ViewError>::Ok(air::Tree::empty()));
    }
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
    let bytes = Literal::byte_string(&bytes);
    let (tn, b) = (Ident::new("__AIR_T", Span::mixed_site()), Ident::new("__air_b", Span::mixed_site()));
    let mut names = vec![];
    node_captures(nodes, &mut names);
    let c = clones(&names);
    quote!({
        #c
        ::core::result::Result::<air::Tree, air::ViewError>::Ok(air::Tree::new(move || {
            static #tn: air::__rt::Template = air::__rt::Template { nodes: #bytes, count: #count, roots: #roots, marks: #marks, first: #first, last: #last };
            let #b = air::__rt::tpl(&#tn);
            #(#holes)*
            air::__rt::view(&#tn, #b)
        }))
    })
}

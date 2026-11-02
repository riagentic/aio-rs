//! A page's `<head>` as its components ask for it (aio's `air/head.ts`): the
//! title and the `<meta>`/`<link>` tags each owner wants, merged. Plain
//! data; the shell writes the merged head to the window
//! (`use_head`).

/// One tag's attributes, in order: `[("name", "description"), ("content", "…")]`.
/// An empty value is a bare attribute.
pub type Tag = Vec<(String, String)>;

/// What one owner (a page, a layout) wants in `<head>`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Head {
    /// The title while it owns the head; the innermost owner's wins.
    pub title: Option<String>,
    /// One per identity (`name`, `property`, `http-equiv`, `charset` or
    /// `itemprop`, with its value): a page's `description` replaces a layout's.
    pub meta: Vec<Tag>,
    /// One per `rel` + `href`; one per page for `canonical`, `manifest`, `icon`.
    pub link: Vec<Tag>,
}

impl Head {
    pub fn title(t: impl Into<String>) -> Self {
        Self { title: Some(t.into()), ..Default::default() }
    }

    /// With `<meta {k}="{v}" …>`.
    pub fn meta(mut self, attrs: &[(&str, &str)]) -> Self {
        self.meta.push(tag(attrs));
        self
    }

    /// With `<link {k}="{v}" …>`.
    pub fn link(mut self, attrs: &[(&str, &str)]) -> Self {
        self.link.push(tag(attrs));
        self
    }
}

fn tag(attrs: &[(&str, &str)]) -> Tag {
    attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The merged head: the title (none: the page's own) and the tags, `meta`
/// or `link` with their attributes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Merged {
    pub title: Option<String>,
    pub tags: Vec<(&'static str, Tag)>,
}

/// The live owners of a window's head, by the order they were first seen:
/// views render outside in, so that is depth order, and a layout that
/// re-renders keeps its place (it never takes the page's title).
#[derive(Clone, Debug, Default)]
pub struct Heads {
    next: u64,
    owners: std::collections::BTreeMap<u64, Head>,
}

impl Heads {
    /// A new owner's place.
    pub fn owner(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// Owner `id` wants `head` now; true if that changed anything.
    pub fn set(&mut self, id: u64, head: Head) -> bool {
        self.owners.insert(id, head.clone()) != Some(head)
    }

    /// Owner `id` is gone.
    pub fn remove(&mut self, id: u64) {
        self.owners.remove(&id);
    }

    /// Later owners win: the title, and a tag of the same identity, which
    /// takes the first one's place (a layout's order, a page's value). A tag
    /// with an attribute name no document can hold is left out, loudly.
    pub fn merged(&self) -> Merged {
        let mut title = None;
        let mut tags: Vec<(String, (&'static str, Tag))> = vec![];
        for h in self.owners.values() {
            title = h.title.clone().or(title);
            let all = h.meta.iter().map(|t| ("meta", t)).chain(h.link.iter().map(|t| ("link", t)));
            for (kind, t) in all {
                if let Some((bad, _)) = t.iter().find(|(k, _)| !attr_name(k)) {
                    eprintln!("[aio] head: <{kind}> attribute {bad:?} is not an attribute name; the tag is left out");
                    continue;
                }
                let key = identity(kind, t);
                match tags.iter_mut().find(|(k, _)| *k == key) {
                    Some(slot) => slot.1 = (kind, t.clone()),
                    None => tags.push((key, (kind, t.clone()))),
                }
            }
        }
        Merged { title, tags: tags.into_iter().map(|(_, t)| t).collect() }
    }
}

/// A tag's identity: which attribute names a `<meta>` and what it says
/// (`name=description` and `itemprop=description` are two tags); a `<link>`
/// by `rel` and `href`. A `<meta>` with none of those is its whole self.
fn identity(kind: &str, t: &Tag) -> String {
    let get = |a: &str| t.iter().find(|(k, _)| k.eq_ignore_ascii_case(a)).map(|(_, v)| v.as_str());
    if kind == "meta" {
        let by = ["name", "property", "http-equiv", "charset", "itemprop"].into_iter().find_map(|a| Some((a, get(a)?)));
        return by.map_or_else(|| format!("meta:{t:?}"), |(a, v)| format!("meta:{a}={v}"));
    }
    match get("rel").unwrap_or("") {
        rel @ ("canonical" | "manifest" | "icon") => format!("link:{rel}"),
        rel => format!("link:{rel}:{}", get("href").unwrap_or("")),
    }
}

/// The merged tags as markup for a page's `<head>` (a first paint on the
/// server), each marked aio's as the page's head writer marks them (pure).
pub fn html(m: &Merged) -> String {
    let esc = |v: &str| v.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;");
    m.tags
        .iter()
        .map(|(kind, attrs)| {
            let a: String = attrs.iter().map(|(k, v)| format!(" {k}=\"{}\"", esc(v))).collect();
            format!("<{kind}{a} data-aio-head>")
        })
        .collect()
}

/// What a view owns of the head: a [`Head`], or a closure making one from
/// state (`move || Head::title(format!("{} · Shop", name.get()))`), followed.
pub trait HeadOf: 'static {
    fn head(&self) -> Head;
}

impl HeadOf for Head {
    fn head(&self) -> Head {
        self.clone()
    }
}

impl<F: Fn() -> Head + 'static> HeadOf for F {
    fn head(&self) -> Head {
        self()
    }
}

/// Own part of the page's `<head>` while this view lives (aio's `useHead`):
/// the innermost owner's title (the window's title follows it) and the
/// tags, merged by identity ([`Heads`]); the last owner gone gives the page
/// its own title back. A hook (call it at a view's top level); a closure
/// is followed, the page written only when the merged head changed.
#[cfg(feature = "kit")]
pub fn use_head(head: impl HeadOf) {
    use std::{cell::RefCell, rc::Rc};
    type Shared = Rc<RefCell<Heads>>;
    let heads = crate::ui::try_context::<Shared>().unwrap_or_else(|| air::provide_root(Shared::default()));
    let id = crate::ui::once(|| heads.borrow_mut().owner());
    let h = heads.clone();
    crate::ui::effect(move || {
        let head = head.head();
        if h.borrow_mut().set(id, head) {
            write(&h.borrow().merged());
        }
    });
    crate::ui::on_cleanup(move || {
        heads.borrow_mut().remove(id);
        write(&heads.borrow().merged());
    });
}

/// `m` written to the page: through its DOM in a browser (its policy runs
/// no code from the app), as a page script in a window.
#[cfg(feature = "kit")]
fn write(m: &Merged) {
    #[cfg(target_arch = "wasm32")]
    write_page(m);
    #[cfg(not(target_arch = "wasm32"))]
    air::exec(&head_js(m));
}

/// `m` written to the page as [`head_js`] writes it, through its DOM.
#[cfg(all(target_arch = "wasm32", feature = "kit"))]
fn write_page(m: &Merged) {
    thread_local! {
        /// The page's own title while an owner sets one.
        static OWN: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    }
    let Some(d) = web_sys::window().and_then(|w| w.document()) else { return };
    match &m.title {
        Some(t) => {
            OWN.with_borrow_mut(|own| _ = own.get_or_insert_with(|| d.title()));
            d.set_title(t);
        }
        None => {
            if let Some(own) = OWN.with_borrow_mut(Option::take) {
                d.set_title(&own);
            }
        }
    }
    let Some(head) = d.head() else { return };
    if let Ok(old) = head.query_selector_all("[data-aio-head]") {
        (0..old.length()).filter_map(|i| old.item(i)).for_each(|n| _ = head.remove_child(&n));
    }
    for (kind, attrs) in &m.tags {
        let Ok(e) = d.create_element(kind) else { continue };
        attrs.iter().for_each(|(k, v)| _ = e.set_attribute(k, v));
        _ = e.set_attribute("data-aio-head", "");
        _ = head.append_child(&e);
    }
}

/// The script writing `m` to the page: the title (the page's own kept, and
/// restored when no owner sets one) and aio's tags (`data-aio-head`)
/// replaced; tags the page came with are never touched.
#[cfg(any(all(feature = "kit", not(target_arch = "wasm32")), test))]
fn head_js(m: &Merged) -> String {
    let tags: Vec<_> = m.tags.iter().map(|(kind, attrs)| serde_json::json!([kind, attrs])).collect();
    format!(
        "const h={},d=document;\
         if(h.title!==null){{if(d.__aioTitle===undefined)d.__aioTitle=d.title;d.title=h.title}}\
         else if(d.__aioTitle!==undefined){{d.title=d.__aioTitle;delete d.__aioTitle}}\
         for(const e of d.head.querySelectorAll('[data-aio-head]'))e.remove();\
         for(const [k,attrs] of h.tags){{const e=d.createElement(k);for(const [n,v] of attrs)e.setAttribute(n,v);\
         e.setAttribute('data-aio-head','');d.head.appendChild(e)}}",
        serde_json::json!({ "title": m.title, "tags": tags })
    )
}

/// An attribute name a document can hold (HTML: no space, quote, `>`, `/`,
/// `=` or control): a name made from data must not write markup.
pub fn attr_name(k: &str) -> bool {
    !k.is_empty() && k.chars().all(|c| !c.is_control() && !c.is_whitespace() && !"\"'>/=<".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_innermost_owner_has_the_title_and_unmounting_hands_it_back() {
        // aio tests/use-head.test.tsx.
        let mut hs = Heads::default();
        let (layout, page) = (hs.owner(), hs.owner());
        hs.set(layout, Head::title("Blog").meta(&[("name", "description"), ("content", "all posts")]));
        hs.set(page, Head::title("Post").meta(&[("name", "description"), ("content", "one post")]));
        let m = hs.merged();
        assert_eq!(m.title.as_deref(), Some("Post"));
        assert_eq!(m.tags, [("meta", tag(&[("name", "description"), ("content", "one post")]))]);
        // The layout re-renders: it keeps its place, the page keeps the title.
        assert!(!hs.set(layout, Head::title("Blog").meta(&[("name", "description"), ("content", "all posts")])));
        hs.set(layout, Head::title("Blog (1)"));
        assert_eq!(hs.merged().title.as_deref(), Some("Post"));
        hs.remove(page);
        assert_eq!(hs.merged().title.as_deref(), Some("Blog (1)"));
        hs.remove(layout);
        assert_eq!(hs.merged(), Merged::default(), "the page's own title is back");
        // A page that owns only tags leaves the title to its layout.
        let (l, p) = (hs.owner(), hs.owner());
        hs.set(l, Head::title("L"));
        hs.set(p, Head::default().link(&[("rel", "canonical"), ("href", "/p")]));
        assert_eq!(hs.merged().title.as_deref(), Some("L"));
    }

    #[test]
    fn a_meta_is_named_by_its_attribute_and_its_value() {
        // aio tests/use-head-meta-identity.test.tsx.
        let mut hs = Heads::default();
        let id = hs.owner();
        hs.set(
            id,
            Head::default()
                .meta(&[("name", "description"), ("content", "a")])
                .meta(&[("itemprop", "description"), ("content", "b")])
                .meta(&[("property", "description"), ("content", "c")])
                .meta(&[("http-equiv", "refresh"), ("content", "5")])
                .meta(&[("NAME", "description"), ("content", "d")]),
        );
        let m = hs.merged();
        let contents: Vec<&str> = m.tags.iter().map(|(_, t)| t[1].1.as_str()).collect();
        assert_eq!(contents, ["d", "b", "c", "5"], "three descriptions; the later one by name takes the first place");
    }

    #[test]
    fn links_are_one_per_rel_and_href_and_canonical_one_per_page() {
        let mut hs = Heads::default();
        let (a, b) = (hs.owner(), hs.owner());
        hs.set(
            a,
            Head::default().link(&[("rel", "canonical"), ("href", "/a")]).link(&[("rel", "preload"), ("href", "/x")]),
        );
        hs.set(
            b,
            Head::default().link(&[("rel", "canonical"), ("href", "/b")]).link(&[("rel", "preload"), ("href", "/y")]),
        );
        let m = hs.merged();
        let hrefs: Vec<&str> = m.tags.iter().map(|(_, t)| t[1].1.as_str()).collect();
        assert_eq!(hrefs, ["/b", "/x", "/y"]);
    }

    #[test]
    fn an_attribute_name_that_would_write_markup_is_refused() {
        // aio tests/use-head-attr-name-guard.test.tsx.
        let mut hs = Heads::default();
        let id = hs.owner();
        hs.set(
            id,
            Head::default().meta(&[("x onload=alert(1)//", "1"), ("name", "a")]).meta(&[
                ("name", "ok"),
                ("data-x", ""),
                ("aria-label", "y"),
            ]),
        );
        let m = hs.merged();
        assert_eq!(m.tags.len(), 1);
        assert_eq!(m.tags[0].1[0], ("name".into(), "ok".into()));
        for bad in ["", "a b", "a\"", "a>", "a/", "a=b", "a'", "\u{7}"] {
            assert!(!attr_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_first_paints_head_is_markup_with_its_values_kept_in_their_attributes() {
        let mut hs = Heads::default();
        let id = hs.owner();
        hs.set(id, Head::title("T").meta(&[("name", "description"), ("content", "\"><script>x")]));
        assert_eq!(
            html(&hs.merged()),
            r#"<meta name="description" content="&quot;&gt;&lt;script&gt;x" data-aio-head>"#
        );
    }

    /// The head written to a document, in Node: the page's own title back
    /// when no owner sets one, its own tags untouched, aio's replaced.
    #[test]
    fn the_merged_head_is_written_and_the_pages_title_comes_back() {
        let mut hs = Heads::default();
        let (layout, page) = (hs.owner(), hs.owner());
        hs.set(layout, Head::title("Blog").meta(&[("name", "description"), ("content", "all")]));
        hs.set(page, Head::title("</script>\"Post\"").link(&[("rel", "canonical"), ("href", "/p")]));
        let set = super::head_js(&hs.merged());
        hs.remove(page);
        let back = super::head_js(&hs.merged());
        hs.remove(layout);
        let gone = super::head_js(&hs.merged());
        let js = format!(
            "class El {{ constructor(k) {{ this.k = k; this.a = []; }} setAttribute(n, v) {{ this.a.push(n + '=' + v); }}
               remove() {{ head.kids = head.kids.filter((e) => e !== this); }} }}
             const head = {{ kids: [Object.assign(new El('meta'), {{ own: true }})],
               querySelectorAll: () => head.kids.filter((e) => e.a.includes('data-aio-head=')),
               appendChild: (e) => head.kids.push(e) }};
             globalThis.document = {{ title: 'Own', head, createElement: (k) => new El(k) }};
             const show = () => document.title + ' | ' + head.kids.map((e) => e.own ? 'own' : e.k + ':' + e.a.join(',')).join(' ');
             const out = [];
             (() => {{ {set} }})(); out.push(show());
             (() => {{ {set} }})(); out.push(show());
             (() => {{ {back} }})(); out.push(show());
             (() => {{ {gone} }})(); out.push(show());
             console.log(JSON.stringify(out));"
        );
        let Ok(out) = std::process::Command::new("node").args(["-e", &js]).output() else {
            return eprintln!("skipped: no node");
        };
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let got: Vec<String> = serde_json::from_slice(&out.stdout).unwrap();
        let meta = "meta:name=description,content=all,data-aio-head=";
        assert_eq!(
            got,
            [
                format!("</script>\"Post\" | own {meta} link:rel=canonical,href=/p,data-aio-head="),
                format!("</script>\"Post\" | own {meta} link:rel=canonical,href=/p,data-aio-head="),
                format!("Blog | own {meta}"),
                "Own | own".to_string(),
            ]
        );
    }
}

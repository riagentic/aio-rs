//! A view rendered to HTML on the server (`air::ssr`): the markup a page
//! shows before its script runs, which parses back into the nodes air
//! builds (so the client hydrates it). Each markup is checked against the
//! page the client builds, settled (`TestPage::markup`): the server's own
//! renderer and the client's edits write the same HTML.
use air::testing::TestPage;
use air::{ErrorBoundary, Suspense, View, component, resource, rsx, signal};

/// `app`'s markup from the server, which is the client's settled page's.
fn render_to_string(app: fn() -> View) -> Result<String, String> {
    let html = air::ssr::render_to_string(app)?;
    assert_eq!(html, TestPage::mount(app).markup(), "the server's markup is the client's");
    Ok(html)
}

#[test]
fn a_view_renders_to_its_markup_with_its_state_its_attributes_and_no_listener() {
    fn app() -> View {
        let n = signal(|| 3u32);
        rsx! {
            main { class: "board",
                h1 { "data-t": "count", "{n} items" }
                button { "data-t": "add", disabled: n.get() > 5, onclick: move |_| n += 1, "Add" }
                ul { for i in 0..n.get() { li { key: "{i}", "row {i}" } } }
            }
        }
    }
    // Each hole's place is a comment: the client's nodes, one for one.
    assert_eq!(
        render_to_string(app).unwrap(),
        "<!----><main class=\"board\"><h1 data-t=\"count\">3 items</h1><button data-t=\"add\">Add</button>\
         <ul><li>row 0</li><li>row 1</li><li>row 2</li><!----></ul></main><!---->"
    );
}

#[test]
fn a_text_html_would_merge_or_drop_is_marked() {
    fn app() -> View {
        let (a, b) = (signal(String::new), signal(|| "b".to_string()));
        rsx! { p { "a" {b.get()} } p { {a.get()} } }
    }
    let html = render_to_string(app).unwrap();
    // One after another text, one empty: both kept as nodes of their own.
    assert!(html.contains("<p>a<!--a-->b<!---->") && html.contains("<p><!--a--><!---->"), "{html}");
    assert_eq!(html.matches(air::ssr::TEXT_MARK).count(), 2, "{html}");
}

#[test]
fn text_and_attributes_are_escaped_and_an_attribute_name_that_would_write_markup_is_left_out() {
    fn app() -> View {
        let said = "<script>alert('x')</script> & \"q\"".to_string();
        let attributes = vec![air::Attribute::new("x\"><script>", "1"), air::Attribute::new("data-ok", "a\"b<c")];
        rsx! { p { title: "{said}", ..attributes, "{said}" } }
    }
    let html = render_to_string(app).unwrap();
    assert!(html.contains("&lt;script&gt;alert('x')&lt;/script&gt; &amp; \"q\""), "{html}");
    assert!(html.contains("title=\"<script>alert('x')</script> &amp; &quot;q&quot;\""), "{html}");
    assert!(html.contains("data-ok=\"a&quot;b<c\"") && !html.contains("x\""), "{html}");
}

#[test]
fn a_controls_properties_are_written_as_what_shows_them_before_any_script() {
    fn app() -> View {
        rsx! {
            input { value: "typed \"in\"", r#type: "text" }
            input { r#type: "checkbox", checked: true }
            input { r#type: "checkbox", checked: false }
            textarea { value: "a <b>" }
            select { option { value: "1", "one" } option { value: "2", selected: true, "two" } }
            br {}
            div { dangerous_inner_html: "<b>bold</b>" }
        }
    }
    let html = render_to_string(app).unwrap();
    for want in [
        "<input value=\"typed &quot;in&quot;\" type=\"text\">",
        "<input type=\"checkbox\" checked=\"\">",
        "<input type=\"checkbox\">",
        "<textarea>a &lt;b&gt;</textarea>",
        "<option value=\"2\" selected=\"\">two</option>",
        "<br>",
        "<div><b>bold</b></div>",
    ] {
        assert!(html.contains(want), "{want} in {html}");
    }
    assert!(!html.contains("</input>") && !html.contains("</br>"), "void elements have no end tag: {html}");
}

#[test]
fn an_svg_and_a_style_keep_their_markup() {
    fn app() -> View {
        rsx! {
            style { "a > b {{ color: red }}" }
            svg { view_box: "0 0 2 2", circle { cx: "1", cy: "1", r: "1" } }
        }
    }
    let html = render_to_string(app).unwrap();
    assert!(html.contains("<style>a > b { color: red }</style>"), "raw text, not escaped: {html}");
    assert!(html.contains("<svg viewBox=\"0 0 2 2\"><circle cx=\"1\" cy=\"1\" r=\"1\"></circle></svg>"), "{html}");
}

#[test]
fn a_caught_error_renders_its_fallback_and_an_uncaught_one_is_the_error() {
    #[component]
    fn Fails() -> View {
        Err(std::io::Error::other("the disk is gone"))?;
        rsx! { p { "fine" } }
    }
    let caught = || rsx! { ErrorBoundary { fallback: |e: String| rsx! { p { "{e}" } }, Fails {} } };
    assert!(render_to_string(caught).unwrap().contains("<p>the disk is gone</p>"));
    let err = air::ssr::render_to_string(|| rsx! { Fails {} }).unwrap_err();
    assert!(err.contains("the disk is gone"), "{err}");
}

#[test]
fn ready_work_settles_before_the_paint_and_waiting_work_shows_its_fallback() {
    #[component]
    fn Ready() -> View {
        let r = resource(|| async { 41 });
        rsx! { p { {r.get().map(|v| v.to_string()).unwrap_or_default()} } }
    }
    #[component]
    fn Waits() -> View {
        let r = resource(std::future::pending::<u32>);
        let v = r.suspend()?;
        rsx! { p { "{v}" } }
    }
    let app = || {
        rsx! {
            Ready {}
            Suspense { fallback: rsx! { p { "loading" } }, Waits {} }
        }
    };
    let html = render_to_string(app).unwrap();
    assert!(html.contains("<p>41<!----></p>") && html.contains("<p>loading</p>"), "{html}");
}

#[test]
fn every_kind_of_hole_writes_the_same_markup_from_the_server_as_the_client_shows() {
    #[component]
    fn Row(n: u32, children: View) -> View {
        if n == 2 {
            Err(std::io::Error::other("row 2 is broken"))?;
        }
        rsx! { li { class: if n % 2 == 0 { "even" } else { "odd" }, "row " {n} {children} } }
    }
    #[component]
    fn Loaded() -> View {
        let r = resource(|| async { "loaded".to_string() });
        let v = r.suspend()?;
        rsx! { b { "{v}" } }
    }
    fn app() -> View {
        let items = signal(|| vec![3u32, 1, 4]);
        let on = signal(|| true);
        let note = signal(|| None::<String>);
        let later = signal(|| 0u32);
        air::effect(move || later.set(7));
        let pick = 2u32;
        rsx! {
            section { style: "color: red; margin: 0", "data-on": on.get(), hidden: false,
                ul {
                    for n in 0..4u32 {
                        ErrorBoundary { key: "{n}", fallback: |e: String| rsx! { li { "{e}" } }, Row { n, i { "#" } } }
                    }
                }
                ol { for n in items.get() { li { key: "{n}", "{n}" } } }
                if on.get() { p { "on" } } else { p { "off" } }
                if let Some(t) = note.get() { p { "{t}" } } else { p { "no note" } }
                match pick { 1 => p { "one" }, n => p { "many: {n}" } }
                {air::styled(rsx! { em { "faded" } }, "opacity", move || on.get().then(|| "0.5".to_string()))}
                {air::text_view(move || format!("after {}", later.get()))}
                {air::text_view(|| "plain".to_string())}
                p { "a" {"b"} {None::<View>} {later.get()} }
                div { dangerous_inner_html: "<i>raw</i>" }
                Suspense { fallback: rsx! { p { "loading" } }, Loaded {} }
                svg { view_box: "0 0 1 1", path { d: "M0 0" } }
            }
        }
    }
    let html = render_to_string(app).unwrap();
    assert!(html.contains("<li>row 2 is broken</li>") && html.contains("<li class=\"odd\">row "), "{html}");
    assert!(html.contains("<b>loaded</b>") && html.contains("after 7") && html.contains("opacity: 0.5"), "{html}");
}

/// A value the test hands a waiting future (from any thread).
#[derive(Clone, Default)]
struct Gate(std::sync::Arc<std::sync::Mutex<(Option<u32>, Option<std::task::Waker>)>>);

impl Gate {
    fn open(&self, v: u32) {
        let w = {
            let mut g = self.0.lock().unwrap();
            g.0 = Some(v);
            g.1.take()
        };
        if let Some(w) = w {
            w.wake();
        }
    }

    fn wait(&self) -> impl std::future::Future<Output = u32> + use<> {
        let g = self.clone();
        std::future::poll_fn(move |cx| {
            let mut g = g.0.lock().unwrap();
            match g.0 {
                Some(v) => std::task::Poll::Ready(v),
                None => {
                    g.1 = Some(cx.waker().clone());
                    std::task::Poll::Pending
                }
            }
        })
    }
}

thread_local!(static GATES: std::cell::RefCell<Vec<Gate>> = Default::default());

/// The gate the `n`th load waits on.
fn gate(n: usize) -> Gate {
    GATES.with_borrow_mut(|g| {
        g.resize_with(g.len().max(n + 1), Gate::default);
        g[n].clone()
    })
}

fn seeded(n: usize) -> air::Resource<u32> {
    air::seeded_resource(|v: &u32| Some(v.to_string()), |s| s.parse().ok(), move || gate(n).wait())
}

#[component]
fn Count(n: usize) -> View {
    let v = seeded(n).suspend()?;
    rsx! { b { "{v} items" } }
}

#[component]
fn Plain() -> View {
    let v = resource(|| gate(9).wait()).suspend()?;
    rsx! { i { "{v}" } }
}

/// What the page holds once `part`'s swaps are in place (as `SWAP_JS`
/// does), and the seeded values its data blocks carry.
fn swapped(mut page: String, part: &str, seeds: &mut Vec<(String, String)>) -> String {
    let mut rest = part;
    while let Some(at) = rest.find("<template data-aio-swap=\"") {
        let after = &rest[at + 25..];
        let (n, after) = after.split_once("\">").unwrap();
        let (kids, after) = after.split_once("</template>").unwrap();
        let (a, b) = (format!("<!--s:{n}-->"), format!("<!--/s:{n}-->"));
        let (i, j) = (page.find(&a).unwrap(), page.find(&b).unwrap());
        page.replace_range(i..j + b.len(), &format!("<!---->{kids}<!---->"));
        rest = after;
    }
    for block in part.split("<script type=\"application/json\" data-aio-data>{").skip(1) {
        let json = &block[..block.find("}</script>").unwrap()];
        for kv in json.split(",\"") {
            let (k, v) = kv.rsplit_once(':').unwrap();
            seeds.push((k.trim_matches('"').to_string(), v.to_string()));
        }
    }
    page
}

/// `page` with its boundaries' marks as plain comments.
fn unmarked(page: &str) -> String {
    let mut out = String::new();
    for (i, part) in page.split("<!--").enumerate() {
        out.push_str(if i == 0 { "" } else { "<!--" });
        let end = part.find("-->").unwrap_or(0);
        let marked = part.starts_with("s:") || part.starts_with("/s:");
        out.push_str(if marked { &part[end..] } else { part });
    }
    out
}

#[test]
fn a_streamed_page_sends_its_waiting_boundaries_as_they_load_and_the_client_starts_from_their_values() {
    fn app() -> View {
        rsx! {
            h1 { "Board" }
            Suspense { fallback: rsx! { p { "counting" } },
                Count { n: 0 }
                Suspense { fallback: rsx! { p { "counting more" } }, Count { n: 1 } }
            }
            Suspense { fallback: rsx! { p { "plain" } }, Plain {} }
        }
    }
    GATES.with_borrow_mut(Vec::clear);
    let (html, (), mut s) = air::ssr::stream(app, || ()).unwrap();
    assert!(
        html.contains("<h1>Board</h1>") && html.contains("<!--s:6--><p>counting</p><!--/s:6-->"),
        "fallbacks first: {html}"
    );
    assert!(!html.contains("counting more"), "an inner boundary is the outer's children's: {html}");
    assert_eq!(html.matches("<!--s:").count(), 2, "{html}");
    assert!(s.waiting() && s.ready().is_empty(), "nothing loaded yet");

    let mut seeds = vec![];
    gate(0).open(41);
    let first = s.ready();
    assert!(first.contains("<b>41 items</b>") && first.contains("<p>counting more</p><!--/s:"), "{first}");
    let page = swapped(html, &first, &mut seeds);
    assert!(s.waiting(), "the inner boundary still waits");
    // A plain resource's value lands on the client, not in the stream.
    gate(9).open(5);
    gate(1).open(2);
    let second = s.ready();
    assert!(second.contains("<b>2 items</b>") && !second.contains("<i>5</i>"), "{second}");
    let page = swapped(page, &second, &mut seeds);
    assert!(!s.waiting() && s.ready().is_empty(), "all sent");
    assert_eq!(seeds.len(), 2, "{seeds:?}");
    drop(s);

    // The client's first frame, from the values the page carried: the page as streamed.
    GATES.with_borrow_mut(Vec::clear);
    for (k, v) in seeds {
        air::ssr::seed(k, v);
    }
    let client = TestPage::mount(app);
    // A boundary still waiting keeps its marks (hydration writes a comment's text).
    assert_eq!(client.markup(), unmarked(&page));
    assert!(client.markup().contains("<p>plain</p>"), "the plain one loads on the client");
}

#[test]
fn a_stream_waits_no_longer_than_its_deadline_and_not_for_plain_resources() {
    fn plain() -> View {
        rsx! { Suspense { fallback: rsx! { p { "plain" } }, Plain {} } }
    }
    GATES.with_borrow_mut(Vec::clear);
    let (_, (), s) = air::ssr::stream(plain, || ()).unwrap();
    assert!(!s.waiting(), "a plain resource is the client's");
    drop(s);

    fn counted() -> View {
        rsx! { Suspense { fallback: rsx! { p { "counting" } }, Count { n: 3 } } }
    }
    let (_, (), s) = air::ssr::stream(counted, || ()).unwrap();
    let t0 = std::time::Instant::now();
    let mut parts = vec![];
    s.finish(t0 + std::time::Duration::from_millis(50), |p| parts.push(p));
    assert!(parts.is_empty() && t0.elapsed() >= std::time::Duration::from_millis(50), "{parts:?}");

    // A load done on another thread wakes the stream at once.
    let (_, (), s) = air::ssr::stream(counted, || ()).unwrap();
    let g = gate(3);
    let opener = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        g.open(8);
    });
    let t0 = std::time::Instant::now();
    let mut parts = vec![];
    s.finish(t0 + std::time::Duration::from_secs(10), |p| parts.push(p));
    opener.join().unwrap();
    assert!(t0.elapsed() < std::time::Duration::from_secs(5), "woken, not timed out");
    assert!(parts.concat().contains("<b>8 items</b>"), "{parts:?}");
}

#[test]
fn what_runs_beside_the_app_leaves_its_markup_the_servers() {
    fn app() -> View {
        rsx! { h1 { "Board" } p { "a" } }
    }
    #[component]
    fn Tooling() -> View {
        let n = signal(|| 0u32);
        air::effect(move || n.set(1));
        rsx! { for i in 0..n.get() { i { key: "{i}", "tool" } } }
    }
    thread_local!(static RAN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    let root = air::Root::new();
    root.show(app);
    root.beside(|| {
        RAN.set(true);
        rsx! { Tooling {} }
    });
    let page = TestPage::on(root);
    assert!(RAN.get(), "it runs");
    assert_eq!(page.markup(), render_to_string(app).unwrap(), "the page holds the app alone");
}

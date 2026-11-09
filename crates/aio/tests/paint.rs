//! A served page's first paint (aio's `renderToString` with `collectHead`):
//! the view rendered on the server for one viewer, at the address asked,
//! its cells as that viewer's socket shows them, its head and the seed the
//! page's copies start from.
use aio::http::{Http, Viewer};
use aio::prelude::*;
use aio::serde_json::{Value, json};
use aio::ui::prelude::*;
use aio::ui::{Seed, paint};

aio::client!(Notes, notes);
aio::router!();

aio::routes! {
    enum Page {
        Home = "/" => "Notes",
        Note { id: usize } = "/notes/:id" => "Note {id}",
    }
}

fn mine(n: &Notes, viewer: &Caller) -> Notes {
    let me = match &viewer.by {
        By::User(u) => u.as_str(),
        _ => "",
    };
    Notes { items: n.items.iter().filter(|(who, _)| who == me).cloned().collect(), ..n.clone() }
}

#[cell(shared, visible = mine)]
pub struct Notes {
    /// (author, text)
    pub items: Vec<(String, String)>,
    #[server]
    pub audit: Hidden<Vec<String>>,
}

#[methods]
impl Notes {
    #[client(predict = false)]
    pub fn add(&mut self, who: String, text: String) -> Result {
        self.audit.push(format!("{who} wrote"));
        self.items.push((who, text));
        Ok(())
    }
}

fn app() -> View {
    use_live();
    let page = memo(|| Page::parse(&here().path));
    aio::head::use_head(move || {
        page.get().map(|p| p.head()).unwrap_or_default().meta(&[("name", "description"), ("content", "my notes")])
    });
    let items = part(|n| &n.items);
    rsx! {
        h1 { "data-t": "page", {format!("{:?}", page.get())} }
        ul { for (i, (_, t)) in items.get().into_iter().enumerate() { li { key: "{i}", "{t}" } } }
    }
}

fn written() -> Handle<Notes> {
    let h = Handle::<Notes>::memory();
    h.add("ann".into(), "ann's".into()).unwrap();
    h.add("bo".into(), "bo's".into()).unwrap();
    h
}

/// The seed a painted page carries, read back.
fn seed_of(head: &str) -> Value {
    let at = head.find("<script type=\"application/json\" id=\"aio-seed\">").expect("a seed");
    let json = &head[at..].split_once('>').unwrap().1;
    aio::serde_json::from_str(json.split_once("</script>").unwrap().0).unwrap()
}

#[test]
fn a_paint_shows_the_viewers_cells_at_the_address_asked_with_its_head_and_its_seed() {
    let h = written();
    let ann = Viewer { user: Some("ann".into()), role: None, linked: true, path: "/notes/3?x=1".into() };
    let p = paint(app, &ann, Seed::new(&ann).cell(&h)).unwrap();
    assert!(p.body.contains("Some(Note { id: 3 })"), "the router starts where it was asked: {}", p.body);
    assert!(p.body.contains("<li>ann's</li>") && !p.body.contains("bo's"), "her own, by `visible`: {}", p.body);
    assert_eq!(p.title.as_deref(), Some("Note 3"));
    assert!(p.head.starts_with("<meta name=\"description\" content=\"my notes\" data-aio-head>"), "{}", p.head);
    // What her socket would send first: the projection, no #[server] field.
    let seed = seed_of(&p.head);
    assert_eq!(seed["notes"], json!({ "seq": 2, "state": { "items": [["ann", "ann's"]] }, "user": "ann" }));
    // A visitor who may not link (accounts, signed out) sees no cell at all.
    let visitor = Viewer { path: "/".into(), ..Default::default() };
    let p = paint(app, &visitor, Seed::new(&visitor).cell(&h)).unwrap();
    assert!(p.body.contains("Some(Home)") && !p.body.contains("<li>"), "{}", p.body);
    assert_eq!((p.title.as_deref(), seed_of(&p.head)), (Some("Notes"), json!({})));
}

#[test]
fn a_seed_ends_no_script_and_a_failed_view_is_the_error() {
    let h = Handle::<Notes>::memory();
    h.add("peer".into(), "</script><script>alert(1)</script>".into()).unwrap();
    let key = Viewer { linked: true, path: "/".into(), ..Default::default() };
    let p = paint(app, &key, Seed::new(&key).cell(&h)).unwrap();
    assert_eq!(p.head.matches("</script>").count(), 1, "{}", p.head);
    assert_eq!(seed_of(&p.head)["notes"]["state"]["items"][0][1], "</script><script>alert(1)</script>");
    fn broken() -> View {
        Err(std::io::Error::other("no disk"))?;
        rsx! {}
    }
    assert!(paint(broken, &key, Seed::new(&key)).unwrap_err().contains("no disk"));
}

const KEY: &str = "a-long-random-app-key-for-tests";

/// One HTTP/1.1 GET with `headers`: the status and the body.
async fn get(addr: &str, path: &str, headers: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{headers}Connection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut got = Vec::new();
    s.read_to_end(&mut got).await.unwrap();
    let text = String::from_utf8_lossy(&got).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    (head[9..12].parse().unwrap(), body.into())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_served_shell_is_painted_for_each_navigation_with_what_its_socket_shows() {
    let dir = tempfile::tempdir().unwrap();
    let shell = "<!doctype html><html><head><title>Notes app</title></head><body><div id=\"main\"></div></body></html>";
    std::fs::write(dir.path().join("index.html"), shell).unwrap();
    let h = written();
    h.add("peer".into(), "the key's".into()).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let http = Http::default().shell(dir.path()).painted(app);
    let opts = aio::sync::Options { token: KEY.into(), http, ..Default::default() };
    tokio::spawn(aio::sync::serve_with(h.clone(), l, opts));
    let (status, page) = get(&addr, &format!("/notes/1?token={KEY}"), "Sec-Fetch-Mode: navigate\r\n").await;
    assert_eq!(status, 200);
    assert!(page.contains("<div id=\"main\" data-aio-painted><!----><h1 data-t=\"page\">"), "{page}");
    assert!(page.contains("<li>the key's</li>") && !page.contains("ann's"), "the key holder is `peer`: {page}");
    assert!(page.contains("<title>Note 1</title>") && page.contains("#aio-splash{display:none}"), "{page}");
    // The seed is what the key holder's socket sends first.
    let copy = aio::sync::Remote::<Notes>::connect(&addr, KEY, "peer").await.unwrap();
    let seed = seed_of(&page);
    let theirs: Notes = aio::serde_json::from_value(seed["notes"]["state"].clone()).unwrap();
    assert_eq!(theirs.items, copy.confirmed().items);
    assert_eq!(seed["notes"]["seq"], json!(3));
    // A script's fetch of the page (the worker's offline copy) is the shell as built.
    let (_, fetched) = get(&addr, &format!("/?token={KEY}"), "Sec-Fetch-Mode: cors\r\n").await;
    assert_eq!(fetched, shell);
    // Without the key: nothing.
    assert_eq!(get(&addr, "/notes/1", "").await.0, 401);
}

#[component]
fn Counted() -> View {
    let n = seeded_resource(|| async {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        vec!["<b>".to_string()]
    })
    .suspend()?;
    rsx! { p { "data-t": "counted", "{n.len()} counted" } }
}

fn waits() -> View {
    rsx! {
        h1 { "Notes" }
        Suspense { fallback: rsx! { p { "counting" } }, Counted {} }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_painted_page_whose_views_wait_is_streamed_its_scripts_nonced_and_filled_in_as_they_load() {
    let dir = tempfile::tempdir().unwrap();
    let shell = "<!doctype html><html><head><title>Notes app</title></head><body><div id=\"main\"></div>\
                 <script src=\"./page.js\"></script></body></html>";
    std::fs::write(dir.path().join("index.html"), shell).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let http = Http::default().shell(dir.path()).painted(waits);
    let security = aio::security::Security { csp: "strict".into(), nonce: true, ..Default::default() };
    let opts = aio::sync::Options { token: KEY.into(), http, security, ..Default::default() };
    tokio::spawn(aio::sync::serve_with(written(), l, opts));
    let (head, parts) = streamed(&addr, &format!("/?token={KEY}")).await;
    assert!(head.to_ascii_lowercase().contains("transfer-encoding: chunked"), "{head}");
    let page = parts.concat();
    // The page first, its fallback marked, the shell's script nonced as the policy says.
    let csp = head.lines().find(|l| l.to_ascii_lowercase().starts_with("content-security-policy:")).unwrap();
    let nonce = csp.split("'nonce-").nth(1).unwrap().split('\'').next().unwrap();
    assert!(page.contains(&format!("<script nonce=\"{nonce}\" src=\"./page.js\">")), "{page}");
    let first = page.find("<p>counting</p>").expect("the fallback first");
    assert!(page[..first].contains("<!--s:") && page.contains("<h1>Notes</h1>"), "{page}");
    // Then the view that waited, before the page's end, with the value its page starts from.
    let swap = page.find("<template data-aio-swap=").expect("a swap");
    let filled = "<p data-t=\"counted\">1 counted</p><!----></template><script type=\"application/json\" \
                  data-aio-data>{\"Suspense#0/Counted#0:0\":[\"\\u003cb>\"]}</script></body>";
    assert!(first < swap && page[swap..].contains(filled), "{page}");
    assert!(parts.len() > 1, "sent in parts: {parts:?}");
}

/// One HTTP/1.1 GET: the head, and the body's chunks as they came.
async fn streamed(addr: &str, path: &str) -> (String, Vec<String>) {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nSec-Fetch-Mode: navigate\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut r = tokio::io::BufReader::new(s);
    let (mut head, mut line) = (String::new(), String::new());
    while r.read_line(&mut line).await.unwrap() > 2 {
        head.push_str(&std::mem::take(&mut line));
    }
    let mut parts = vec![];
    loop {
        line.clear();
        r.read_line(&mut line).await.unwrap();
        let n = usize::from_str_radix(line.trim(), 16).unwrap();
        if n == 0 {
            return (head, parts);
        }
        let mut b = vec![0; n + 2];
        r.read_exact(&mut b).await.unwrap();
        parts.push(String::from_utf8(b[..n].to_vec()).unwrap());
    }
}

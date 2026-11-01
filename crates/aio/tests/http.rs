use aio::http::{CookieOpts, Http, Reply, Route};
use aio::prelude::*;
use std::collections::BTreeMap;

#[cell(persist, shared)]
pub struct Hooks {
    pub paid: BTreeMap<String, u64>,
}

#[methods]
impl Hooks {
    pub fn pay(&mut self, order: String, cents: u64) -> Result {
        ensure!(cents > 0, "nothing paid");
        self.paid.insert(order, cents);
        Ok(())
    }
}

const KEY: (&str, &str) = ("Authorization", "Bearer secret");
const JSON: (&str, &str) = ("Content-Type", "application/json");

async fn serve(h: Handle<Hooks>, http: Http) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(aio::sync::serve_with(
        h,
        l,
        aio::sync::Options { token: "secret".into(), http, ..Default::default() },
    ));
    addr
}

/// One plain HTTP/1.1 request; the status, the head (lowercase) and the body.
async fn fetch(addr: &str, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String, String) {
    let (status, head, body) = fetch_raw(addr, method, path, headers, body).await;
    (status, head, String::from_utf8(body).unwrap())
}

/// [`fetch`] with the body as bytes, unchunked.
async fn fetch_raw(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, Vec<u8>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let extra: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    let at = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(out[..at].to_vec()).unwrap().to_ascii_lowercase();
    let body = out.split_off(at + 4);
    let body = if head.contains("transfer-encoding: chunked") { unchunk(&body) } else { body };
    (head[9..12].parse().unwrap(), head, body)
}

fn unchunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let at = b.windows(2).position(|w| w == b"\r\n").unwrap();
        let n = usize::from_str_radix(std::str::from_utf8(&b[..at]).unwrap().trim(), 16).unwrap();
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&b[at + 2..at + 2 + n]);
        b = &b[at + 4 + n..];
    }
}

/// One header's value in a lowercased head.
fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix(": "))
}

fn routes(h: &Handle<Hooks>) -> Http {
    let h = h.clone();
    Http::default()
        .route(Route::get("/api/items/:id", |r| async move {
            let seen = CookieOpts { path: Some("/".into()), ..Default::default() };
            let id = r.param("id").unwrap_or_default().to_string();
            Reply::json(
                &serde_json::json!({ "id": id, "method": r.method(), "q": r.query("x"), "sid": r.cookie("sid") }),
            )
            .cookie("seen", &id, &seen)
        }))
        .route(
            Route::post("/hooks/pay", move |r| {
                let h = h.clone();
                async move {
                    #[derive(serde::Deserialize)]
                    struct Paid {
                        order: String,
                        cents: u64,
                    }
                    match r.json::<Paid>().and_then(|p| h.pay(p.order, p.cents)) {
                        Ok(()) => Reply::text("charged"),
                        Err(e) => Reply::text(e.why).status(400),
                    }
                }
            })
            .public(),
        )
        .route(Route::get("/boom", |_| async { panic!("handler exploded") }))
        .route(Route::get("/ok", |_| async { Reply::text("fine") }))
        .route(Route::get("/login", |_| async {
            Reply::redirect("/dashboard").cookie("sid", "abc", &CookieOpts { http_only: true, ..Default::default() })
        }))
        .route(Route::get("/*", |r| async move { Reply::text(format!("SPA:{}", r.param("*").unwrap_or_default())) }))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_param_route_answers_json_with_its_cookie() {
    let addr = serve(Handle::memory(), routes(&Handle::memory())).await;
    let (status, head, body) =
        fetch(&addr, "GET", "/api/items/abc?x=1", &[KEY, ("Cookie", "sid=s%201; other=1")], "").await;
    assert_eq!(status, 200);
    assert!(head.contains("content-type: application/json; charset=utf-8"), "{head}");
    assert!(head.contains("set-cookie: seen=abc; path=/"), "{head}");
    assert!(head.contains("x-content-type-options: nosniff"), "the gate's headers: {head}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v, serde_json::json!({ "id": "abc", "method": "GET", "q": "1", "sid": "s 1" }));
    // HEAD rides with GET; any other method is told what is allowed.
    assert_eq!(fetch(&addr, "HEAD", "/api/items/abc", &[KEY], "").await.0, 200);
    let (status, head, _) = fetch(&addr, "DELETE", "/api/items/abc", &[KEY], "").await;
    assert_eq!(status, 405);
    assert!(head.contains("allow: get, head"), "{head}");
    // A redirect keeps its cookie.
    let (status, head, _) = fetch(&addr, "GET", "/login", &[KEY], "").await;
    assert_eq!(status, 302);
    assert!(head.contains("location: /dashboard") && head.contains("set-cookie: sid=abc; httponly"), "{head}");
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_need_the_key_unless_public() {
    let h = Handle::<Hooks>::memory();
    let addr = serve(h.clone(), routes(&h)).await;
    let (status, _, body) = fetch(&addr, "GET", "/ok", &[], "").await;
    assert_eq!((status, body.as_str()), (401, "wrong or missing key"));
    assert_eq!(fetch(&addr, "GET", "/ok", &[("Authorization", "Bearer nope")], "").await.0, 401);
    // A link with the key hands the browser the key cookie for what follows.
    let (status, head, _) = fetch(&addr, "GET", "/ok?token=secret", &[], "").await;
    assert_eq!(status, 200);
    let line = head.lines().find(|l| l.starts_with("set-cookie: aio_key_hooks=")).expect(&head).to_string();
    assert!(line.contains("httponly") && line.contains("samesite=strict") && line.contains("path=/"), "{line}");
    let (status, head, body) = fetch(&addr, "GET", "/ok", &[("Cookie", "aio_key_hooks=secret")], "").await;
    assert_eq!((status, body.as_str()), (200, "fine"));
    assert!(!head.contains("set-cookie"), "{head}");
    // A public webhook answers without the key and changes the cell.
    let json = ("Content-Type", "application/json");
    let (status, _, body) = fetch(&addr, "POST", "/hooks/pay", &[json], r#"{"order":"o1","cents":500}"#).await;
    assert_eq!((status, body.as_str()), (200, "charged"));
    assert_eq!(h.state().paid["o1"], 500);
    assert_eq!(fetch(&addr, "POST", "/hooks/pay", &[json], r#"{"order":"o2","cents":0}"#).await.0, 400);
    // The gate stands in front of every route: a form post, a foreign host.
    let form = ("Content-Type", "application/x-www-form-urlencoded");
    assert_eq!(fetch(&addr, "POST", "/hooks/pay", &[form], "order=o3").await.0, 415);
    assert_eq!(fetch(&addr, "POST", "/hooks/pay", &[json, ("Origin", "http://evil.example")], "{}").await.0, 403);
    // Too big a body is said, not cut.
    let big = format!(r#"{{"order":"{}","cents":1}}"#, "x".repeat(aio::http::MAX_BODY));
    assert_eq!(fetch(&addr, "POST", "/hooks/pay", &[json], &big).await.0, 413);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_catch_all_never_swallows_aios_paths_and_a_panic_fails_one_request() {
    let addr = serve(Handle::memory(), routes(&Handle::memory())).await;
    let (status, _, body) = fetch(&addr, "GET", "/deep/link", &[KEY], "").await;
    assert_eq!((status, body.as_str()), (200, "SPA:deep/link"));
    for p in ["/__aio/health", "/__aio/pin", "/ws"] {
        let (status, _, body) = fetch(&addr, "GET", p, &[KEY], "").await;
        assert!(!body.starts_with("SPA:"), "{p} was captured: {status} {body}");
    }
    let (status, _, body) = fetch(&addr, "GET", "/boom", &[KEY], "").await;
    assert_eq!((status, body.as_str()), (500, "Internal Server Error"));
    let (status, _, body) = fetch(&addr, "GET", "/ok", &[KEY], "").await;
    assert_eq!((status, body.as_str()), (200, "fine"), "still serving");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shell_and_extra_dirs_are_served_read_only() {
    let t = tempfile::tempdir().unwrap();
    let (dist, shared) = (t.path().join("dist"), t.path().join("shared"));
    std::fs::create_dir_all(&dist).unwrap();
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(dist.join("index.html"), "<html>shell</html>").unwrap();
    std::fs::write(dist.join("notes.txt"), "hello").unwrap();
    std::fs::write(dist.join(".env"), "SECRET=1").unwrap();
    std::fs::write(shared.join("sse.js"), "parse()").unwrap();
    let http = Http::default()
        .route(Route::post("/api/upload", |r| async move { Reply::text(format!("routed {}", r.method())) }))
        .shell(&dist)
        .dir("/shared", &shared);
    let addr = serve(Handle::memory(), http).await;
    let (status, head, body) = fetch(&addr, "GET", "/notes.txt", &[KEY], "").await;
    assert_eq!((status, body.as_str()), (200, "hello"));
    assert!(head.contains("content-type: text/plain; charset=utf-8"), "{head}");
    let (status, head, body) = fetch(&addr, "GET", "/shared/sse.js", &[KEY], "").await;
    assert_eq!((status, body.as_str()), (200, "parse()"));
    assert!(head.contains("content-type: text/javascript; charset=utf-8"), "{head}");
    let (status, head, body) = fetch(&addr, "GET", "/some/client/route", &[KEY], "").await;
    assert_eq!((status, body.as_str()), (200, "<html>shell</html>"));
    assert!(head.contains("cache-control: no-cache"), "{head}");
    for p in ["/.env", "/missing.js", "/shared/../dist/.env", "/notes.txt/"] {
        assert_eq!(fetch(&addr, "GET", p, &[KEY], "").await.0, 404, "{p}");
    }
    assert_eq!(fetch(&addr, "GET", "/notes.txt", &[], "").await.0, 401, "files need the key too");
    // A write to a file or a client route is 405, never the file or the shell.
    let json = ("Content-Type", "application/json");
    for p in ["/", "/notes.txt", "/api/uplaod", "/some/client/route"] {
        for m in ["POST", "PUT", "DELETE", "PATCH"] {
            let (status, head, _) = fetch(&addr, m, p, &[KEY, json], "{}").await;
            assert_eq!(status, 405, "{m} {p}");
            assert!(head.contains("allow: get, head"), "{m} {p}: {head}");
        }
        assert_eq!(fetch(&addr, "HEAD", p, &[KEY], "").await.0, 200, "HEAD {p}");
    }
    let (status, _, body) = fetch(&addr, "POST", "/api/upload", &[KEY, json], "{}").await;
    assert_eq!((status, body.as_str()), (200, "routed POST"));
    // The framework's namespace keeps its own answers.
    assert_eq!(fetch(&addr, "POST", "/__aio/nope", &[KEY, json], "{}").await.0, 404);
}

async fn serve_opts(bind: &str, opts: aio::sync::Options) -> u16 {
    let l = tokio::net::TcpListener::bind(bind).await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(aio::sync::serve_with(Handle::<Hooks>::memory(), l, opts));
    port
}

#[tokio::test(flavor = "multi_thread")]
async fn aio_toml_serves_dirs_from_the_files_tier_and_public_ones_without_the_key() {
    let t = tempfile::tempdir().unwrap();
    let (files, own) = (t.path().join("files"), t.path().join("own"));
    std::fs::create_dir_all(files.join("media")).unwrap();
    std::fs::create_dir_all(files.join("brand")).unwrap();
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(files.join("media/song.mp3"), "la").unwrap();
    std::fs::write(files.join("brand/logo.svg"), "<svg/>").unwrap();
    std::fs::write(own.join("a.txt"), "operator's").unwrap();
    let toml = format!(
        "[app]\nid = \"t\"\n[server.dirs]\n\"/media\" = \"media\"\n\"/own\" = {:?}\n[server.public_dirs]\n\"/brand\" = \"brand\"",
        own.display().to_string()
    );
    let cfg = aio::config::Config::parse(&toml).unwrap();
    let addr = serve(Handle::memory(), Http::default().configured(&cfg.server, &files)).await;
    assert_eq!(fetch(&addr, "GET", "/media/song.mp3", &[KEY], "").await.2, "la");
    assert_eq!(fetch(&addr, "GET", "/own/a.txt", &[KEY], "").await.2, "operator's");
    assert_eq!(fetch(&addr, "GET", "/media/song.mp3", &[], "").await.0, 401, "behind the key");
    assert_eq!(fetch(&addr, "GET", "/brand/logo.svg", &[], "").await.2, "<svg/>", "public");
    assert_eq!(fetch(&addr, "GET", "/brand/../media/song.mp3", &[], "").await.0, 404, "no way out of it");
}

#[tokio::test(flavor = "multi_thread")]
async fn built_in_assets_are_served_at_their_paths_to_anyone() {
    static FILES: &[(&str, &[u8])] = &[("/media/a%20b.svg", b"<svg/>"), ("/media/song.mp3", b"la")];
    let addr = serve(Handle::memory(), Http::default().assets(FILES)).await;
    let (status, headers, body) = fetch(&addr, "GET", "/media/song.mp3", &[], "").await;
    assert_eq!((status, body.as_str()), (200, "la"), "they ship with the app: no key");
    assert!(headers.contains("content-type: audio/mpeg"), "{headers}");
    assert_eq!(fetch(&addr, "GET", "/media/a%20b.svg", &[], "").await.2, "<svg/>");
    assert_eq!(fetch(&addr, "GET", "/media/other.mp3", &[], "").await.0, 401, "only what is built in");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_open_app_needs_no_key_but_minting_a_pin_still_does() {
    let opts = |open| aio::sync::Options {
        token: "secret".into(),
        http: routes(&Handle::memory()),
        pairing: Some(aio::pairing::pairing()),
        open,
        ..Default::default()
    };
    let socket = async |port: u16, q: &str| match tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?wire=2{q}"
    ))
    .await
    {
        Ok(_) => 101,
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => r.status().as_u16(),
        Err(e) => panic!("{e}"),
    };
    let open = serve_opts("127.0.0.1:0", opts(true)).await;
    let addr = format!("127.0.0.1:{open}");
    assert_eq!(fetch(&addr, "GET", "/ok", &[], "").await.2, "fine");
    assert_eq!(socket(open, "").await, 101);
    assert_eq!(socket(open, "&token=wrong").await, 101);
    assert_eq!(fetch(&addr, "POST", "/__aio/pin", &[JSON], "{}").await.0, 403, "a PIN pairs a device: the key's");
    assert_eq!(fetch(&addr, "POST", "/__aio/pin", &[KEY, JSON], "{}").await.0, 200);
    // The default is the key.
    let shut = serve_opts("127.0.0.1:0", opts(false)).await;
    assert_eq!(fetch(&format!("127.0.0.1:{shut}"), "GET", "/ok", &[], "").await.0, 401);
    assert_eq!(socket(shut, "").await, 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn behind_a_trusted_proxy_the_client_is_its_last_hop() {
    let xff = ("X-Forwarded-For", "198.51.100.9, 203.0.113.7");
    let opts = |trust: Option<&str>| aio::sync::Options {
        token: "secret".into(),
        http: Http::default().route(Route::get("/ip", |r| async move { Reply::text(r.ip().to_string()) })),
        trust_proxy: trust.map(String::from),
        ..Default::default()
    };
    let trusted = format!("127.0.0.1:{}", serve_opts("127.0.0.1:0", opts(Some("x-forwarded-for"))).await);
    assert_eq!(fetch(&trusted, "GET", "/ip", &[KEY, xff], "").await.2, "203.0.113.7");
    assert_eq!(fetch(&trusted, "GET", "/ip", &[KEY], "").await.2, "127.0.0.1");
    // Untrusted, the header is the client's word: the peer stays the client.
    let plain = format!("127.0.0.1:{}", serve_opts("127.0.0.1:0", opts(None)).await);
    assert_eq!(fetch(&plain, "GET", "/ip", &[KEY, xff], "").await.2, "127.0.0.1");
    // The key cookie is `Secure` when the trusted proxy says the browser is on https.
    let cookie = |head: String| head.lines().find(|l| l.starts_with("set-cookie: aio_key_hooks=")).unwrap().to_string();
    let https = ("X-Forwarded-Proto", "https");
    assert!(cookie(fetch(&trusted, "GET", "/ip?token=secret", &[xff, https], "").await.1).contains("; secure"));
    assert!(!cookie(fetch(&trusted, "GET", "/ip?token=secret", &[xff], "").await.1).contains("; secure"));
    assert!(!cookie(fetch(&plain, "GET", "/ip?token=secret", &[https], "").await.1).contains("; secure"));
}

#[tokio::test(flavor = "multi_thread")]
async fn strict_origin_holds_sockets_to_an_exposed_app() {
    use tokio_tungstenite::tungstenite::{Error, client::IntoClientRequest};
    let opts = || aio::sync::Options { token: "secret".into(), strict_origin: true, ..Default::default() };
    let open = async |port: u16, origin: Option<String>| {
        let mut req = format!("ws://127.0.0.1:{port}/ws?token=secret&wire=2").into_client_request().unwrap();
        if let Some(o) = origin {
            req.headers_mut().insert("origin", o.parse().unwrap());
        }
        match tokio_tungstenite::connect_async(req).await {
            Ok(_) => 101,
            Err(Error::Http(r)) => r.status().as_u16(),
            Err(e) => panic!("{e}"),
        }
    };
    let exposed = serve_opts("0.0.0.0:0", opts()).await;
    assert_eq!(open(exposed, None).await, 403);
    assert_eq!(open(exposed, Some(format!("http://127.0.0.1:{exposed}"))).await, 101);
    assert_eq!(open(exposed, Some("http://evil.example".into())).await, 403);
    // Loopback only: tools without an Origin are fine.
    assert_eq!(open(serve_opts("127.0.0.1:0", opts()).await, None).await, 101);
    // Plain HTTP is not a socket: no Origin needed.
    assert_eq!(fetch(&format!("127.0.0.1:{exposed}"), "GET", "/nothing", &[KEY], "").await.0, 404);
}

const BYTES: &str = "0123456789abcdefghij";

#[tokio::test(flavor = "multi_thread")]
async fn a_route_serves_a_file_with_ranges_tags_and_inert_types() {
    let t = tempfile::tempdir().unwrap();
    for name in ["clip.mp4", "a.bin", "evil.html"] {
        std::fs::write(t.path().join(name), BYTES).unwrap();
    }
    let dir = t.path().to_path_buf();
    let typed = t.path().join("evil.html");
    let http = Http::default()
        .route(Route::get("/media/:name", move |r| {
            let dir = dir.clone();
            async move {
                // The base name only: a param may hold `/` or `..`.
                let name = std::path::Path::new(r.param("name").unwrap_or_default()).file_name().unwrap_or_default();
                aio::http::serve_file(&r, dir.join(name), None).await
            }
        }))
        .route(Route::new(&["GET", "POST"], "/typed", move |r| {
            let p = typed.clone();
            async move { aio::http::serve_file(&r, p, Some("text/plain")).await }
        }));
    let addr = serve(Handle::memory(), http).await;
    let (status, head, body) = fetch(&addr, "GET", "/media/clip.mp4", &[KEY], "").await;
    assert_eq!((status, body.as_str()), (200, BYTES));
    for (k, v) in [
        ("content-length", "20"),
        ("accept-ranges", "bytes"),
        ("content-type", "video/mp4"),
        ("x-content-type-options", "nosniff"),
        ("content-security-policy", "sandbox"),
        ("cache-control", "no-cache, no-transform"),
    ] {
        assert_eq!(header(&head, k), Some(v), "{k}: {head}");
    }
    // A renderable type is never inferred; one the app names is sent.
    let typ = async |p: &str| header(&head_of(&addr, p).await, "content-type").map(String::from);
    assert_eq!(typ("/media/evil.html").await.as_deref(), Some("application/octet-stream"));
    assert_eq!(typ("/typed").await.as_deref(), Some("text/plain"));
    for (range, status, cr, want) in [
        ("bytes=0-4", 206, Some("bytes 0-4/20"), "01234"),
        ("bytes=15-", 206, Some("bytes 15-19/20"), "fghij"),
        ("bytes=-3", 206, Some("bytes 17-19/20"), "hij"),
        ("bytes=18-500", 206, Some("bytes 18-19/20"), "ij"),
        ("bytes=5-2", 200, None, BYTES),
        ("bytes=0-1,4-5", 200, None, BYTES),
    ] {
        let (s, head, body) = fetch(&addr, "GET", "/media/a.bin", &[KEY, ("Range", range)], "").await;
        assert_eq!((s, header(&head, "content-range"), body.as_str()), (status, cr, want), "{range}");
        assert_eq!(header(&head, "content-length"), Some(want.len().to_string().as_str()), "{range}");
    }
    for range in ["bytes=20-", "bytes=-0", "bytes=99-100"] {
        let (s, head, body) = fetch(&addr, "GET", "/media/a.bin", &[KEY, ("Range", range)], "").await;
        assert_eq!(
            (s, header(&head, "content-range"), body.as_str()),
            (416, Some("bytes */20"), "Range Not Satisfiable")
        );
        assert_eq!(
            header(&head, "content-type"),
            Some("text/plain; charset=utf-8"),
            "the error's type, not the file's"
        );
    }
    let (s, head, body) = fetch(&addr, "HEAD", "/media/a.bin", &[KEY, ("Range", "bytes=0-1")], "").await;
    assert_eq!((s, header(&head, "content-length"), body.as_str()), (206, Some("2"), ""));
    let json = ("Content-Type", "application/json");
    let (s, head, _) = fetch(&addr, "POST", "/typed", &[KEY, json], "{}").await;
    assert_eq!((s, header(&head, "allow")), (405, Some("get, head")));
    for p in ["/media/nope".to_string(), "/media/..".into(), format!("/media/{}", "x".repeat(300))] {
        assert_eq!(fetch(&addr, "GET", &p, &[KEY], "").await.0, 404, "{p}");
    }
    // The head is lowercased here; a client sends the tag as given.
    let tag = header(&head_of(&addr, "/media/a.bin").await, "etag").unwrap().replacen("w/", "W/", 1);
    assert!(tag.starts_with("W/\""), "{tag}");
    assert_eq!(fetch(&addr, "GET", "/media/a.bin", &[KEY, ("If-None-Match", &tag)], "").await.0, 304);
    // A weak tag never satisfies If-Range: the whole file, never a splice.
    let (s, _, body) =
        fetch(&addr, "GET", "/media/a.bin", &[KEY, ("Range", "bytes=0-1"), ("If-Range", &tag)], "").await;
    assert_eq!((s, body.as_str()), (200, BYTES));
}

async fn head_of(addr: &str, path: &str) -> String {
    fetch_raw(addr, "GET", path, &[KEY], "").await.1
}

#[tokio::test(flavor = "multi_thread")]
async fn static_files_take_ranges_revalidate_and_compress() {
    use std::io::Read;
    let t = tempfile::tempdir().unwrap();
    let js = "aio ".repeat(2000);
    std::fs::write(t.path().join("index.html"), "<html>shell</html>").unwrap();
    std::fs::write(t.path().join("app.js"), &js).unwrap();
    std::fs::write(t.path().join("a.txt"), BYTES).unwrap();
    // Past what is read whole: streamed, with its mtime+size tag.
    let big: Vec<u8> = (0..(9u32 << 20)).map(|i| (i % 251) as u8).collect();
    std::fs::write(t.path().join("big.bin"), &big).unwrap();
    let addr = serve(Handle::memory(), Http::default().shell(t.path())).await;
    let (s, head, body) = fetch(&addr, "GET", "/a.txt", &[KEY, ("Range", "bytes=2-4")], "").await;
    assert_eq!((s, header(&head, "content-range"), body.as_str()), (206, Some("bytes 2-4/20"), "234"));
    assert_eq!(header(&head, "content-type"), Some("text/plain; charset=utf-8"));
    let tag = header(&head_of(&addr, "/a.txt").await, "etag").unwrap().to_string();
    // A matching If-None-Match beats Range: 304, not 206.
    let (s, _, body) = fetch(&addr, "GET", "/a.txt", &[KEY, ("If-None-Match", &tag), ("Range", "bytes=0-1")], "").await;
    assert_eq!((s, body.as_str()), (304, ""));
    let (s, head, body) = fetch_raw(&addr, "GET", "/app.js", &[KEY, ("Accept-Encoding", "gzip")], "").await;
    assert_eq!(
        (s, header(&head, "content-encoding"), header(&head, "vary")),
        (200, Some("gzip"), Some("accept-encoding"))
    );
    assert!(body.len() < js.len() / 2, "{} bytes", body.len());
    let mut back = String::new();
    flate2::read::GzDecoder::new(body.as_slice()).read_to_string(&mut back).unwrap();
    assert_eq!(back, js);
    let tag = header(&head, "etag").unwrap().to_string();
    let (s, head, _) =
        fetch_raw(&addr, "GET", "/app.js", &[KEY, ("Accept-Encoding", "br"), ("If-None-Match", &tag)], "").await;
    assert_eq!((s, header(&head, "content-encoding")), (304, None));
    let br = fetch_raw(&addr, "GET", "/app.js", &[KEY, ("Accept-Encoding", "br")], "").await.1;
    let want = if cfg!(feature = "br") { Some("br") } else { None };
    assert_eq!(header(&br, "content-encoding"), want);
    let (s, head, body) = fetch_raw(&addr, "GET", "/big.bin", &[KEY], "").await;
    assert_eq!((s, body.len()), (200, big.len()));
    assert!(body == big && header(&head, "etag").is_some_and(|t| t.starts_with("w/\"")), "{head}");
    let (s, _, body) = fetch_raw(&addr, "GET", "/big.bin", &[KEY, ("Range", "bytes=9000000-9000003")], "").await;
    assert_eq!((s, body.as_slice()), (206, &big[9_000_000..9_000_004]));
    // Too big to hold, not to compress: a big WASM is gzipped as it is sent.
    std::fs::write(t.path().join("engine.wasm"), &big).unwrap();
    let (s, head, body) = fetch_raw(&addr, "GET", "/engine.wasm", &[KEY, ("Accept-Encoding", "gzip, br")], "").await;
    assert_eq!((s, header(&head, "content-encoding"), header(&head, "content-length")), (200, Some("gzip"), None));
    assert!(body.len() < big.len() / 4, "{} bytes", body.len());
    let mut back = Vec::new();
    flate2::read::GzDecoder::new(body.as_slice()).read_to_end(&mut back).unwrap();
    assert!(back == big, "the bytes are the bytes");
    let (_, head, body) = fetch_raw(&addr, "GET", "/engine.wasm", &[KEY, ("Accept-Encoding", "gzip;q=0")], "").await;
    assert_eq!((header(&head, "content-encoding"), body.len()), (None, big.len()));
    let (_, head, _) = fetch_raw(&addr, "HEAD", "/engine.wasm", &[KEY, ("Accept-Encoding", "gzip")], "").await;
    assert_eq!(header(&head, "content-length"), Some(big.len().to_string().as_str()), "a HEAD keeps its length");
}

/// A chunked POST (no `Content-Length` to trust): its status. A refusal
/// closes the socket with bytes unread: Windows then resets it, and the
/// reply can be lost (`None`); elsewhere it always arrives.
async fn post_chunked(addr: &str, path: &str, chunks: usize, size: usize) -> Option<u16> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer secret\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    );
    s.write_all(head.as_bytes()).await.unwrap();
    let chunk = format!("{size:x}\r\n{}\r\n", "x".repeat(size));
    for _ in 0..chunks {
        // The server may answer and close before the last chunk.
        if s.write_all(chunk.as_bytes()).await.is_err() {
            break;
        }
    }
    _ = s.write_all(b"0\r\n\r\n").await;
    let mut out = Vec::new();
    _ = s.read_to_end(&mut out).await;
    String::from_utf8_lossy(&out).get(9..12).map(|s| s.parse().unwrap())
}

/// Refused as it arrives: 413, or (Windows only) a reset that lost it.
fn refused(status: Option<u16>) -> bool {
    status == Some(413) || (cfg!(windows) && status.is_none())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_is_bounded_by_the_bytes_that_arrive_and_a_route_may_take_more() {
    let len = |r: aio::http::Req| async move { Reply::text(r.body().len().to_string()) };
    let http = Http::default().route(Route::post("/small", len)).route(Route::post("/upload", len).max_body(8 << 20));
    let addr = serve(Handle::memory(), http).await;
    assert_eq!(post_chunked(&addr, "/small", 4, 1 << 10).await, Some(200));
    // Over the limit with no length said: refused as it arrives.
    assert!(refused(post_chunked(&addr, "/small", 40, 64 << 10).await));
    assert_eq!(post_chunked(&addr, "/upload", 40, 64 << 10).await, Some(200));
    assert!(refused(post_chunked(&addr, "/upload", 140, 64 << 10).await));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_streamed_route_pipes_its_body_into_blobs_as_it_arrives() {
    let t = tempfile::tempdir().unwrap();
    let blobs = aio::blobs::Blobs::at(t.path().join("blobs"));
    let (up, ran) = (blobs.clone(), std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)));
    let ran2 = ran.clone();
    let put = Route::post("/upload", move |r| {
        let (up, ran) = (up.clone(), ran2.clone());
        ran.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        async move {
            assert!(r.body().is_empty(), "never buffered");
            let body = r.body_reader().unwrap();
            assert!(r.body_reader().is_none(), "read once");
            match tokio::task::spawn_blocking(move || up.put(body, None)).await.unwrap() {
                Ok(b) => Reply::json(&b).status(201),
                Err(e) => Reply::text(e.to_string()).status(413),
            }
        }
    });
    let http = Http::default().blobs(blobs.clone()).route(put.streamed().max_body(4 << 20));
    let addr = serve(Handle::memory(), http).await;
    // 3 MiB, over the buffered limit, in 64 KiB chunks: stored whole, by its hash.
    assert_eq!(post_chunked(&addr, "/upload", 48, 64 << 10).await, Some(201));
    let same = aio::blobs::Blobs::at(t.path().join("check")).put("x".repeat(48 << 16).as_bytes(), None).unwrap();
    assert_eq!(blobs.list().unwrap(), vec![same.clone()]);
    let (s, _, bytes) = fetch_raw(&addr, "GET", &aio::blobs::url(&same.id), &[KEY], "").await;
    assert_eq!((s, bytes.len()), (200, 48 << 16));
    // Past the limit the read fails; nothing is stored.
    assert!(refused(post_chunked(&addr, "/upload", 80, 64 << 10).await));
    assert_eq!(blobs.list().unwrap().len(), 1);
    // A length said over the limit is refused before the handler runs.
    let before = ran.load(std::sync::atomic::Ordering::SeqCst);
    let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let head = format!(
        "POST /upload HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer secret\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        5 << 20
    );
    tokio::io::AsyncWriteExt::write_all(&mut s, head.as_bytes()).await.unwrap();
    let mut out = [0; 12];
    tokio::io::AsyncReadExt::read_exact(&mut s, &mut out).await.unwrap();
    assert_eq!((&out[9..], ran.load(std::sync::atomic::Ordering::SeqCst)), (&b"413"[..], before));
}

#[tokio::test(flavor = "multi_thread")]
async fn health_and_metrics_answer_a_monitor_without_the_key() {
    let addr = serve(Handle::memory(), Http::default()).await;
    let (s, head, body) = fetch(&addr, "GET", "/__aio/health", &[], "").await;
    assert_eq!((s, header(&head, "cache-control")), (200, Some("no-store")));
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["appId"], "hooks");
    assert_eq!(v["pid"], std::process::id());
    assert!(["healthy", "degraded"].contains(&v["status"].as_str().unwrap()), "{v}");
    let (s, head, body) = fetch(&addr, "GET", "/__aio/metrics", &[], "").await;
    assert_eq!((s, header(&head, "content-type")), (200, Some("text/plain; version=0.0.4")));
    for m in ["# TYPE aio_up gauge", "aio_uptime_seconds ", "aio_memory_rss_bytes ", "aio_clients_connected "] {
        assert!(body.contains(m), "{m} in {body}");
    }
    assert_eq!(fetch(&addr, "HEAD", "/__aio/health", &[], "").await.0, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_snapshot_endpoint_exports_and_loads_the_whole_state_as_aios() {
    let h = Handle::<Hooks>::memory();
    h.call("pay", serde_json::json!(["a1", 5])).unwrap();
    let addr = serve(h.clone(), Http::default()).await;
    // Without the key: refused; with it, this machine's tool gets every cell.
    assert_eq!(fetch(&addr, "GET", "/__aio/snapshot", &[], "").await.0, 401);
    let (s, head, body) = fetch(&addr, "GET", "/__aio/snapshot", &[KEY], "").await;
    assert_eq!((s, header(&head, "cache-control")), (200, Some("no-store")));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap(),
        serde_json::json!({ "hooks": { "paid": { "a1": 5 } } })
    );
    let post = |body: &'static str, path: &'static str, x: bool| {
        let addr = addr.clone();
        async move {
            let headers: Vec<(&str, &str)> = if x { vec![KEY, JSON, ("X-AIO", "1")] } else { vec![KEY, JSON] };
            fetch(&addr, "POST", path, &headers, body).await
        }
    };
    assert_eq!(post(r#"{"hooks":{"paid":{}}}"#, "/__aio/snapshot", false).await.0, 403, "aio's CSRF header");
    let (s, _, why) = post(r#"{"hooks":42}"#, "/__aio/snapshot", true).await;
    assert!(s == 400 && why.contains(r#"cell "hooks" must be an object"#), "{s} {why}");
    let (s, _, why) = post(r#"{"other":{}}"#, "/__aio/snapshot", true).await;
    assert!(s == 400 && why.contains("whose state would be DESTROYED") && why.contains(r#""other""#), "{why}");
    assert_eq!(post("{nope", "/__aio/snapshot", true).await.2, "Invalid JSON");
    assert_eq!(h.state().paid.len(), 1, "nothing refused changed anything");
    assert_eq!(post(r#"{"hooks":{"paid":{"b2":7}}}"#, "/__aio/snapshot", true).await.0, 200);
    assert_eq!(h.state().paid, BTreeMap::from([("b2".into(), 7)]));
    // Forced: a missing cell starts from its defaults, an unknown one is dropped.
    assert_eq!(post(r#"{"other":{}}"#, "/__aio/snapshot?force=1", true).await.0, 200);
    assert!(h.state().paid.is_empty());
    assert_eq!(h.store(|s| s.events().unwrap()).last().unwrap().method, "$rewind", "a new event, the old state kept");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_form_post_navigation_to_a_client_route_gets_the_shell_and_nothing_runs() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("index.html"), "<html>shell</html>").unwrap();
    std::fs::write(t.path().join("notes.txt"), "hello").unwrap();
    let http =
        Http::default().route(Route::post("/api/pay", |_| async { Reply::text("ran") }).public()).shell(t.path());
    let addr = serve(Handle::memory(), http).await;
    let form = ("Content-Type", "application/x-www-form-urlencoded");
    let (nav, dest) = (("Sec-Fetch-Mode", "navigate"), ("Sec-Fetch-Dest", "document"));
    let html = ("Accept", "text/html,application/xhtml+xml,*/*;q=0.8");
    let (s, _, body) = fetch(&addr, "POST", "/checkout/done", &[KEY, form, nav, dest], "status=paid").await;
    assert_eq!((s, body.as_str()), (200, "<html>shell</html>"));
    // No fetch metadata (a plain-http LAN page, older Safari): it asks for HTML.
    let (s, _, body) = fetch(&addr, "POST", "/checkout/done", &[KEY, form, html], "status=paid").await;
    assert_eq!((s, body.as_str()), (200, "<html>shell</html>"));
    // A fetch is no navigation; a route, a file and aio's paths never take a form.
    let cors = [("Sec-Fetch-Mode", "cors"), ("Sec-Fetch-Dest", "empty")];
    assert_eq!(fetch(&addr, "POST", "/checkout/done", &[KEY, form, cors[0], cors[1]], "x").await.0, 415);
    assert_eq!(fetch(&addr, "POST", "/checkout/done", &[KEY, form, ("Accept", "*/*")], "x").await.0, 415);
    let (s, _, body) = fetch(&addr, "POST", "/api/pay", &[form, nav, dest], "x").await;
    assert_eq!((s, body.contains("ran")), (415, false));
    assert_eq!(fetch(&addr, "POST", "/notes.txt", &[KEY, form, nav, dest], "x").await.0, 405);
    assert_eq!(fetch(&addr, "POST", "/__aio/auth/login", &[form, nav, dest], "x").await.0, 415);
    assert_eq!(fetch(&addr, "POST", "/checkout/done", &[form, nav, dest], "x").await.0, 401, "the key, as for GET");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_public_dir_is_read_without_the_key_and_nothing_else_is() {
    let t = tempfile::tempdir().unwrap();
    for d in ["dist", "public", "uploads"] {
        std::fs::create_dir_all(t.path().join(d)).unwrap();
    }
    std::fs::write(t.path().join("dist/index.html"), "<html>shell</html>").unwrap();
    std::fs::write(t.path().join("public/logo.png"), "png").unwrap();
    std::fs::write(t.path().join("public/.env"), "SECRET=1").unwrap();
    std::fs::write(t.path().join("uploads/passport.png"), "private").unwrap();
    let http = Http::default()
        .shell(t.path().join("dist"))
        .public_dir("/public", t.path().join("public"))
        .dir("/uploads", t.path().join("uploads"));
    let addr = serve(Handle::memory(), http).await;
    let (s, _, body) = fetch(&addr, "GET", "/public/logo.png", &[], "").await;
    assert_eq!((s, body.as_str()), (200, "png"));
    for p in ["/uploads/passport.png", "/", "/some/route"] {
        assert_eq!(fetch(&addr, "GET", p, &[], "").await.0, 401, "{p}");
    }
    // Public is not weaker: dotfiles and escapes are still refused.
    for p in ["/public/.env", "/public/../uploads/passport.png", "/public/%2e%2e/uploads/passport.png"] {
        assert_eq!(fetch(&addr, "GET", p, &[], "").await.0, 404, "{p}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_address_guessing_keys_is_blocked_but_a_valid_key_is_always_served() {
    let h = Handle::memory();
    let addr = serve(h.clone(), routes(&h)).await;
    let wrong = ("Authorization", "Bearer guess");
    for _ in 0..10 {
        assert_eq!(fetch(&addr, "GET", "/ok", &[wrong], "").await.0, 401);
    }
    let (s, head, body) = fetch(&addr, "GET", "/ok?token=guess", &[], "").await;
    assert_eq!((s, header(&head, "retry-after")), (429, Some("5")), "{body}");
    // Blocked everywhere the key is asked for, but only for wrong keys.
    let upgrade = [
        ("Upgrade", "websocket"),
        ("Connection", "Upgrade"),
        ("Sec-WebSocket-Version", "13"),
        ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
    ];
    assert_eq!(fetch(&addr, "GET", "/ws", &[&upgrade[..], &[wrong]].concat(), "").await.0, 429);
    assert_eq!(fetch(&addr, "GET", "/ok", &[], "").await.0, 401, "a missing key is no guess");
    let (s, _, body) = fetch(&addr, "GET", "/ok", &[KEY], "").await;
    assert_eq!((s, body.as_str()), (200, "fine"));
    assert_eq!(
        fetch(&addr, "POST", "/hooks/pay", &[wrong, ("Content-Type", "application/json")], "{}").await.0,
        400,
        "public"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn every_answer_is_hardened_and_a_strict_shell_names_its_scripts_by_nonce() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("index.html"), "<html><script>boot()</script><script src=/app.js></script></html>")
        .unwrap();
    std::fs::write(t.path().join("clip.bin"), BYTES).unwrap();
    let clip = t.path().join("clip.bin");
    let http = || {
        let clip = clip.clone();
        Http::default().shell(t.path()).route(Route::get("/clip", move |r| {
            let clip = clip.clone();
            async move { aio::http::serve_file(&r, clip, None).await }
        }))
    };
    // The defaults: framing by this app only, the basic policy, everywhere.
    let addr = serve(Handle::memory(), http()).await;
    let (_, head, body) = fetch(&addr, "GET", "/", &[KEY], "").await;
    assert_eq!(header(&head, "x-frame-options"), Some("sameorigin"));
    assert!(header(&head, "content-security-policy").unwrap().starts_with("base-uri 'self'; object-src 'none'"));
    assert!(!body.contains("nonce"), "no nonce asked: the shell as it is");
    let (_, head, _) = fetch(&addr, "GET", "/__aio/health", &[], "").await;
    assert_eq!(header(&head, "x-content-type-options"), Some("nosniff"));
    // A served file keeps its own sandbox.
    let (_, head, _) = fetch(&addr, "GET", "/clip", &[KEY], "").await;
    assert_eq!(header(&head, "content-security-policy"), Some("sandbox"));
    // Strict with a nonce, embeddable by a named host.
    let security = aio::security::Security {
        csp: "strict".into(),
        nonce: true,
        permissions: Some("camera=()".into()),
        ..Default::default()
    };
    let opts = aio::sync::Options {
        token: "secret".into(),
        http: http(),
        hosts: vec!["dash.corp".into()],
        security,
        ..Default::default()
    };
    let addr = format!("127.0.0.1:{}", serve_opts("127.0.0.1:0", opts).await);
    let nonce_of = |head: &str| {
        let csp = header(head, "content-security-policy").unwrap();
        csp.split("'nonce-").nth(1).unwrap().split('\'').next().unwrap().to_string()
    };
    let (s, head, body) = fetch(&addr, "GET", "/", &[KEY], "").await;
    // (The head is read lowercased.)
    let n = body.split("nonce=\"").nth(1).unwrap().split('"').next().unwrap();
    assert_eq!((s, nonce_of(&head)), (200, n.to_ascii_lowercase()));
    assert_eq!(body.matches(&format!("<script nonce=\"{n}\"")).count(), 2, "{body}");
    assert!(header(&head, "content-security-policy").unwrap().contains("frame-ancestors 'self' dash.corp:*"));
    assert_eq!((header(&head, "x-frame-options"), header(&head, "etag")), (None, None));
    assert_eq!(
        (header(&head, "cache-control"), header(&head, "permissions-policy")),
        (Some("no-store"), Some("camera=()"))
    );
    assert_ne!(nonce_of(&fetch(&addr, "GET", "/deep/link", &[KEY], "").await.1), n, "a nonce per answer");
    // Off the shell: the strict policy, no nonce.
    let (_, head, _) = fetch(&addr, "GET", "/__aio/health", &[], "").await;
    assert!(
        header(&head, "content-security-policy")
            .unwrap()
            .contains("script-src 'self' blob: 'wasm-unsafe-eval' 'unsafe-inline'")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn blobs_are_served_with_ranges_and_cached_for_good_behind_the_key() {
    let t = tempfile::tempdir().unwrap();
    let blobs = aio::blobs::Blobs::at(t.path().join("blobs"));
    let fox = "The quick brown fox jumps over the lazy dog";
    let b = blobs.put(fox.as_bytes(), Some("fox.txt")).unwrap();
    let up = blobs.clone();
    let http = Http::default().blobs(blobs).route(Route::post("/upload", move |r| {
        let up = up.clone();
        async move {
            match up.put(r.body(), r.query("name")) {
                Ok(b) => Reply::json(&b).status(201),
                Err(e) => Reply::text(e.to_string()).status(500),
            }
        }
    }));
    let addr = serve(Handle::memory(), http).await;
    let url = aio::blobs::url(&b.id);
    let (s, head, body) = fetch(&addr, "GET", &url, &[KEY], "").await;
    assert_eq!((s, body.as_str()), (200, fox));
    assert_eq!(header(&head, "content-length"), Some("43"));
    assert_eq!(header(&head, "content-type"), Some("text/plain"));
    assert_eq!(header(&head, "etag").unwrap(), format!("\"{}\"", b.id));
    assert_eq!(header(&head, "cache-control"), Some("private, max-age=31536000, immutable, no-transform"));
    assert_eq!(
        (header(&head, "accept-ranges"), header(&head, "content-security-policy")),
        (Some("bytes"), Some("sandbox"))
    );
    let (s, head, body) = fetch(&addr, "GET", &url, &[KEY, ("Range", "bytes=4-8")], "").await;
    assert_eq!((s, header(&head, "content-range"), body.as_str()), (206, Some("bytes 4-8/43"), "quick"));
    assert_eq!(fetch(&addr, "GET", &url, &[KEY, ("Range", "bytes=-3")], "").await.2, "dog");
    let (s, head, _) = fetch(&addr, "GET", &url, &[KEY, ("Range", "bytes=43-")], "").await;
    assert_eq!((s, header(&head, "content-range")), (416, Some("bytes */43")));
    let (s, head, body) = fetch(&addr, "HEAD", &url, &[KEY], "").await;
    assert_eq!((s, header(&head, "content-length"), body.as_str()), (200, Some("43"), ""));
    let tag = format!("\"{}\"", b.id);
    assert_eq!(fetch(&addr, "GET", &url, &[KEY, ("If-None-Match", &tag)], "").await.0, 304);
    // No probe surface: unknown and malformed ids are the same 404; bytes are read-only.
    for id in ["0".repeat(64), "not-an-id".into(), "../../etc/passwd".into()] {
        assert_eq!(fetch(&addr, "GET", &format!("/__aio/blobs/{id}"), &[KEY], "").await.0, 404, "{id}");
    }
    let json = ("Content-Type", "application/json");
    assert_eq!(fetch(&addr, "POST", &url, &[KEY, json], "{}").await.0, 405);
    assert_eq!(fetch(&addr, "GET", &url, &[], "").await.0, 401, "behind the key");
    // An upload route puts the body; the bytes come back by their id.
    let (s, _, body) = fetch(&addr, "POST", "/upload?name=a.bin", &[KEY, json], "[1,2,3]").await;
    let up: aio::blobs::Blob = serde_json::from_str(&body).unwrap();
    assert_eq!((s, up.size, up.name.as_deref()), (201, 7, Some("a.bin")));
    let (_, head, body) = fetch(&addr, "GET", &aio::blobs::url(&up.id), &[KEY], "").await;
    assert_eq!((header(&head, "content-type"), body.as_str()), (Some("application/octet-stream"), "[1,2,3]"));
    // Not served unless the app asks.
    let plain = serve(Handle::memory(), Http::default()).await;
    assert_eq!(fetch(&plain, "GET", &url, &[KEY], "").await.0, 404);
}

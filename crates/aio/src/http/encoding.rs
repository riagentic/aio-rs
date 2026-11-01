//! Response encoding (aio's `http-encoding`): validators and compression, in
//! the one place every route and file answer passes.
//!
//! - **Validators**: a 200 to GET/HEAD gets an `ETag` (the handler's, else a
//!   content hash), and a matching `If-None-Match` is a 304 that keeps the
//!   headers (`Set-Cookie` too) and drops the body. `no-store` gets none.
//! - **Compression**: br (feature `br`), gzip or deflate, as the client weighs them (`q=0`
//!   refuses; a token said twice takes its lowest weight), for text, JSON,
//!   XML, SVG, bare fonts and WASM of at least [`MIN_COMPRESS`] bytes, never
//!   when it would grow. `Vary: Accept-Encoding` on every such answer.
//!   A file too big to hold is gzipped as it is sent (chunked).
//! - **Untouched**: anything but a 200, an encoded body, `no-transform`, a
//!   stream type (SSE, NDJSON, multipart), an empty HEAD.
use super::Reply;
use std::io::Write;
use std::sync::Mutex;

/// Below about one packet there is nothing to win.
pub const MIN_COMPRESS: usize = 860;
/// Brotli quality: smaller and faster than gzip at 5 (aio measured).
#[cfg(feature = "br")]
const BROTLI_QUALITY: u32 = 5;
/// Compressed bodies kept for tagged answers (files), source and result.
const MEMO_BYTES: usize = 32 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Encoding {
    #[cfg(feature = "br")]
    Br,
    Gzip,
    Deflate,
}

impl Encoding {
    const ALL: &[Encoding] = &[
        #[cfg(feature = "br")]
        Encoding::Br,
        Encoding::Gzip,
        Encoding::Deflate,
    ];

    fn name(self) -> &'static str {
        match self {
            #[cfg(feature = "br")]
            Self::Br => "br",
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
        }
    }
}

fn base(content_type: &str) -> String {
    content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

/// Worth compressing: text and the formats that are text, bare fonts, WASM.
pub fn compressible(content_type: &str) -> bool {
    let ct = base(content_type);
    let app = |t: &str| {
        ["wasm", "javascript", "ecmascript", "json", "xml", "manifest", "toml", "yaml", "x-yaml", "sql", "rtf"]
            .contains(&t)
            || t.ends_with("+json")
            || t.ends_with("+xml")
    };
    ct.starts_with("text/")
        || ct.starts_with("image/svg")
        || ((ct.starts_with("font/") || ct.ends_with("/otf") || ct.ends_with("/ttf")) && !ct.contains("woff"))
        || ct.strip_prefix("application/").is_some_and(app)
}

/// A stream by kind, whatever its size: never buffered or touched.
pub fn streaming(content_type: &str) -> bool {
    let ct = base(content_type);
    ["text/event-stream", "application/x-ndjson", "application/jsonl", "application/stream+json"].contains(&ct.as_str())
        || ct.starts_with("multipart/")
}

/// The best encoding `accept` (an `Accept-Encoding`) takes, or `None`: as is.
pub fn negotiate(accept: Option<&str>) -> Option<Encoding> {
    negotiate_among(accept, Encoding::ALL)
}

fn negotiate_among(accept: Option<&str>, offer: &[Encoding]) -> Option<Encoding> {
    let mut q: Vec<(String, f32)> = Vec::new();
    for part in accept?.split(',') {
        let mut it = part.split(';');
        let name = it.next().unwrap_or("").trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        let w = it
            .filter_map(|p| p.split_once('=').filter(|(k, _)| k.trim().eq_ignore_ascii_case("q")))
            .map(|(_, v)| v.trim().parse::<f32>().unwrap_or(0.0))
            .next_back()
            .unwrap_or(1.0);
        match q.iter_mut().find(|(n, _)| *n == name) {
            Some((_, prev)) => *prev = prev.min(w),
            None => q.push((name, w)),
        }
    }
    let weight = |n: &str| q.iter().find(|(k, _)| k == n).map(|(_, w)| *w);
    let star = weight("*");
    let mut best = None;
    let mut best_q = 0.0;
    for &e in offer {
        let w = weight(e.name()).or(star).unwrap_or(0.0);
        if w > best_q {
            (best, best_q) = (Some(e), w);
        }
    }
    best
}

pub fn compress(e: Encoding, bytes: &[u8]) -> Vec<u8> {
    let out = Vec::with_capacity(bytes.len() / 2);
    let written = match e {
        #[cfg(feature = "br")]
        Encoding::Br => {
            let mut w = brotli::CompressorWriter::new(out, 4096, BROTLI_QUALITY, 22);
            w.write_all(bytes).map(|()| w.into_inner())
        }
        Encoding::Gzip => {
            let mut w = flate2::write::GzEncoder::new(out, flate2::Compression::default());
            w.write_all(bytes).and_then(|()| w.finish())
        }
        Encoding::Deflate => {
            let mut w = flate2::write::ZlibEncoder::new(out, flate2::Compression::default());
            w.write_all(bytes).and_then(|()| w.finish())
        }
    };
    // Writing to memory never fails; a body that can't be had stays as is.
    written.unwrap_or_else(|_| bytes.to_vec())
}

/// A strong tag for `bytes`: its length and 64 bits of its SHA-256.
pub fn etag_of(bytes: &[u8]) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, bytes);
    let hex: String = d.as_ref()[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("\"{:x}-{hex}\"", bytes.len())
}

/// Whether `if_none_match` names `etag` (lists, `W/` and `*` included).
pub fn etag_matches(if_none_match: Option<&str>, etag: &str) -> bool {
    let Some(h) = if_none_match else { return false };
    let bare = |t: &str| t.trim().trim_start_matches("W/").to_string();
    h.trim() == "*" || h.split(',').any(|p| bare(p) == bare(etag))
}

/// `vary` with `name` added once (`*` already covers it).
pub fn merge_vary(vary: Option<&str>, name: &str) -> String {
    match vary.map(str::trim).filter(|v| !v.is_empty()) {
        None => name.into(),
        Some(v) if v == "*" || v.split(',').any(|p| p.trim().eq_ignore_ascii_case(name)) => v.into(),
        Some(v) => format!("{v}, {name}"),
    }
}

/// Compressed bodies by tag and encoding, each checked against its source:
/// two bodies under one tag never get each other's bytes.
type Memo = Vec<((String, Encoding), Vec<u8>, Vec<u8>)>;
static MEMO: Mutex<Memo> = Mutex::new(Vec::new());

fn memo_get(key: &(String, Encoding), src: &[u8]) -> Option<Vec<u8>> {
    let m = MEMO.lock().unwrap_or_else(|e| e.into_inner());
    m.iter().find(|(k, s, _)| k == key && s == src).map(|(_, _, out)| out.clone())
}

fn memo_put(key: (String, Encoding), src: &[u8], out: &[u8]) {
    let mut m = MEMO.lock().unwrap_or_else(|e| e.into_inner());
    m.retain(|(k, _, _)| *k != key);
    m.push((key, src.to_vec(), out.to_vec()));
    // Oldest out first, within the budget.
    while m.iter().map(|(_, s, o)| s.len() + o.len()).sum::<usize>() > MEMO_BYTES && !m.is_empty() {
        m.remove(0);
    }
}

/// `reply` finished for `method` with request headers `req`; `compress`
/// off keeps the validators (a 304 is correctness, not an optimisation).
pub fn encode(method: &str, req: &axum::http::HeaderMap, reply: Reply, compress_on: bool) -> Reply {
    let head = |n: &str| reply.headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    let (ct, cache) = (head("content-type").unwrap_or_default(), head("cache-control").unwrap_or_default());
    let (encoded, handler_tag) = (head("content-encoding").is_some(), head("etag"));
    let vary = merge_vary(head("vary").as_deref(), "Accept-Encoding");
    let squeeze = compressible(&ct);
    // A file's own 304 still says what it varies on.
    if reply.status == 304 && squeeze {
        return reply.header("vary", &vary);
    }
    let untouched = reply.status != 200
        || encoded
        || cache.contains("no-transform")
        || streaming(&ct)
        || (method == "HEAD" && reply.body.is_empty());
    if untouched {
        return reply;
    }
    let accept = req.get("accept-encoding").and_then(|v| v.to_str().ok());
    if let Some(w) = &reply.window {
        // Too big to hold, not too big to compress: gzip as it goes (a HEAD
        // keeps the length it exists to ask for).
        let gzip = compress_on && squeeze && method != "HEAD" && negotiate_among(accept, &[Encoding::Gzip]).is_some();
        if !squeeze || !gzip {
            return if squeeze { reply.header("vary", &vary) } else { reply };
        }
        let window = Some(super::file::Window { gzip: true, ..w.clone() });
        return Reply { window, ..reply }.header("vary", &vary).header("content-encoding", "gzip");
    }
    let no_store = cache.contains("no-store");
    let etag = handler_tag.clone().unwrap_or_else(|| etag_of(&reply.body));
    let mut r = if squeeze { reply.header("vary", &vary) } else { reply };
    let get = method == "GET" || method == "HEAD";
    let inm = req.get("if-none-match").and_then(|v| v.to_str().ok());
    if !no_store {
        r = r.header("etag", &etag);
        if get && etag_matches(inm, &etag) {
            r.headers.retain(|(k, _)| k != "content-length" && k != "content-encoding");
            return Reply { status: 304, body: Vec::new(), ..r };
        }
    }
    if !(compress_on && squeeze && r.body.len() >= MIN_COMPRESS) {
        return r;
    }
    let Some(e) = negotiate(accept) else { return r };
    let key = (etag, e);
    let out = memo_get(&key, &r.body).unwrap_or_else(|| {
        let out = compress(e, &r.body);
        if handler_tag.is_some() {
            memo_put(key, &r.body, &out);
        }
        out
    });
    if out.len() >= r.body.len() {
        return r;
    }
    Reply { body: out, ..r }.header("content-encoding", e.name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use std::io::Read;

    fn big() -> String {
        "aio ".repeat(2000)
    }

    fn js(body: &str) -> Reply {
        Reply::of("text/javascript", body.as_bytes().to_vec()).header("cache-control", "no-cache")
    }

    fn req(kv: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in kv {
            h.insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
        }
        h
    }

    fn get<'a>(r: &'a Reply, name: &str) -> Option<&'a str> {
        r.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn decode(e: &str, b: &[u8]) -> String {
        let mut s = String::new();
        match e {
            #[cfg(feature = "br")]
            "br" => brotli::Decompressor::new(b, 4096).read_to_string(&mut s),
            "gzip" => flate2::read::GzDecoder::new(b).read_to_string(&mut s),
            _ => flate2::read::ZlibDecoder::new(b).read_to_string(&mut s),
        }
        .unwrap();
        s
    }

    #[test]
    fn a_compressible_body_shrinks_and_round_trips_in_every_encoding() {
        for (ae, want) in [
            ("gzip", "gzip"),
            #[cfg(feature = "br")]
            ("gzip, deflate, br", "br"),
            #[cfg(not(feature = "br"))]
            ("gzip, deflate, br", "gzip"),
            ("deflate", "deflate"),
        ] {
            let out = encode("GET", &req(&[("accept-encoding", ae)]), js(&big()), true);
            assert_eq!(get(&out, "content-encoding"), Some(want), "{ae}");
            assert!(out.body.len() < big().len() / 2, "{ae}: {} bytes", out.body.len());
            assert_eq!(decode(want, &out.body), big(), "{ae}");
            assert_eq!(get(&out, "vary"), Some("Accept-Encoding"));
        }
    }

    #[test]
    fn a_client_that_asks_for_nothing_gets_the_bytes_with_vary() {
        for h in [req(&[]), req(&[("accept-encoding", "identity")]), req(&[("accept-encoding", "gzip;q=0")])] {
            let out = encode("GET", &h, js(&big()), true);
            assert_eq!((get(&out, "content-encoding"), out.body.as_slice()), (None, big().as_bytes()));
            assert_eq!(get(&out, "vary"), Some("Accept-Encoding"), "a shared cache must not mix them");
        }
    }

    #[test]
    fn no_cache_revalidates_to_a_304_that_keeps_its_cookie() {
        let first = encode("GET", &req(&[]), js(&big()).cookie("sid", "1", &Default::default()), true);
        let tag = get(&first, "etag").unwrap().to_string();
        let again = js(&big()).cookie("sid", "1", &Default::default());
        let out = encode("GET", &req(&[("if-none-match", &tag), ("accept-encoding", "gzip")]), again, true);
        assert_eq!((out.status, out.body.len()), (304, 0));
        assert_eq!(get(&out, "set-cookie"), Some("sid=1"));
        assert_eq!(get(&out, "content-encoding"), None);
        // A changed body is a new tag; a write is never a 304.
        let changed = encode("GET", &req(&[("if-none-match", &tag)]), js("other"), true);
        assert_eq!(changed.status, 200);
        assert_ne!(get(&changed, "etag"), Some(tag.as_str()));
        assert_eq!(encode("POST", &req(&[("if-none-match", "*")]), js("made"), true).status, 200);
        assert_eq!(encode("HEAD", &req(&[("if-none-match", &tag)]), js(&big()), true).status, 304);
    }

    #[test]
    fn what_must_come_back_untouched_does() {
        let h = req(&[("accept-encoding", "gzip")]);
        let cases = [
            js(&big()).status(201),
            js(&big()).status(206),
            js(&big()).header("content-encoding", "gzip"),
            js(&big()).header("cache-control", "no-transform"),
            Reply::of("text/event-stream", big().into_bytes()),
            Reply::of("application/x-ndjson", big().into_bytes()),
            Reply::of("multipart/mixed", big().into_bytes()),
        ];
        for r in cases {
            assert_eq!(encode("GET", &h, r.clone(), true), r);
        }
        let empty_head = js("").header("content-length", "1234");
        assert_eq!(encode("HEAD", &h, empty_head.clone(), true), empty_head);
        // Incompressible, tiny, or growing: the bytes as they were, tagged.
        for r in [Reply::of("image/png", big().into_bytes()), js("ok")] {
            let out = encode("GET", &h, r.clone(), true);
            assert_eq!((get(&out, "content-encoding"), &out.body), (None, &r.body));
            assert!(get(&out, "etag").is_some());
        }
        let mut noise = vec![0; 4000];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut noise).unwrap();
        let out = encode("GET", &h, Reply::of("application/json", noise.clone()), true);
        assert_eq!((get(&out, "content-encoding"), &out.body), (None, &noise));
    }

    #[test]
    fn no_store_has_no_validator_and_compress_off_keeps_one() {
        let h = req(&[("accept-encoding", "gzip")]);
        let out = encode("GET", &h, js(&big()).header("cache-control", "no-store"), true);
        assert_eq!((get(&out, "etag"), get(&out, "content-encoding")), (None, Some("gzip")));
        let out = encode("GET", &h, js(&big()), false);
        assert_eq!(get(&out, "content-encoding"), None);
        assert!(get(&out, "etag").is_some());
    }

    #[test]
    fn a_handlers_tag_is_used_and_never_mixes_bodies() {
        let h = req(&[("accept-encoding", "gzip")]);
        let (a, b) = ("a".repeat(2000), "b".repeat(2000));
        for _ in 0..2 {
            for body in [&a, &b] {
                let out = encode("GET", &h, js(body).header("etag", "W/\"1-2000\""), true);
                assert_eq!(get(&out, "etag"), Some("W/\"1-2000\""));
                assert_eq!(&decode("gzip", &out.body), body);
            }
        }
    }

    #[test]
    fn types_negotiation_tags_and_vary_follow_aio() {
        for ct in [
            "image/png",
            "image/jpeg",
            "image/webp",
            "font/woff2",
            "application/zip",
            "video/mp4",
            "application/octet-stream",
        ] {
            assert!(!compressible(ct), "{ct}");
        }
        for ct in [
            "text/html; charset=utf-8",
            "text/css",
            "application/wasm",
            "application/javascript",
            "application/json",
            "application/manifest+json",
            "image/svg+xml",
            "application/xml",
            "font/ttf",
        ] {
            assert!(compressible(ct), "{ct}");
        }
        use Encoding::*;
        let table: &[(Option<&str>, Option<Encoding>)] = &[
            #[cfg(feature = "br")]
            (Some("gzip, deflate, br"), Some(Br)),
            #[cfg(feature = "br")]
            (Some("*"), Some(Br)),
            // Without `br` it is never answered, even asked for alone.
            #[cfg(not(feature = "br"))]
            (Some("br"), None),
            #[cfg(not(feature = "br"))]
            (Some("*"), Some(Gzip)),
            (Some("gzip, deflate"), Some(Gzip)),
            (Some("deflate"), Some(Deflate)),
            (Some("*;q=0, gzip"), Some(Gzip)),
            (Some("identity"), None),
            (None, None),
            (Some(""), None),
            (Some("br;q=0.1, gzip;q=0.9"), Some(Gzip)),
            (Some("gzip;q=0, gzip"), None),
            (Some("gzip, gzip;q=0"), None),
            (Some("br;q=0.0, Br"), None),
            (Some("*;q=0, *"), None),
            (Some("br;q=0, gzip"), Some(Gzip)),
            (Some("br ; q = 0.2, gzip;Q=0.5"), Some(Gzip)),
        ];
        for (h, want) in table {
            assert_eq!(negotiate(*h), *want, "{h:?}");
        }
        assert_eq!(etag_of(b"x"), etag_of(b"x"));
        assert_ne!(etag_of(b"x"), etag_of(b"y"));
        let tags: std::collections::HashSet<String> = (0..20_000u32).map(|i| etag_of(&i.to_le_bytes())).collect();
        assert_eq!(tags.len(), 20_000);
        let tag = "\"abc\"";
        for (h, ok) in [
            (Some("\"abc\""), true),
            (Some("W/\"abc\""), true),
            (Some("\"zzz\", \"abc\""), true),
            (Some("*"), true),
            (Some("\"zzz\""), false),
            (None, false),
        ] {
            assert_eq!(etag_matches(h, tag), ok, "{h:?}");
        }
        assert_eq!(merge_vary(Some("Cookie"), "Accept-Encoding"), "Cookie, Accept-Encoding");
        assert_eq!(merge_vary(Some("accept-encoding"), "Accept-Encoding"), "accept-encoding");
        assert_eq!(merge_vary(Some("*"), "Accept-Encoding"), "*");
        assert_eq!(merge_vary(None, "Accept-Encoding"), "Accept-Encoding");
    }
}

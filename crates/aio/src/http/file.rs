//! Files over HTTP (aio's `serveFile` and the static server's Range contract):
//! one range at most (206, or 416 past the end; a malformed or multi-range
//! header gets the whole file), `If-None-Match` → 304, and a file read as it
//! is sent, 64 KiB at a time, closed with its last byte or when the client
//! goes.
use super::{Reply, encoding::etag_matches};
use axum::http::HeaderMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

/// How much is read per step of a streamed file.
const CHUNK: u64 = 64 << 10;

/// What a `Range` header asks of `size` bytes.
#[derive(Debug, PartialEq, Eq)]
pub enum Range {
    /// All of it: no header, or one RFC 9110 says to ignore.
    Whole,
    /// `start..end`, end exclusive.
    Part(u64, u64),
    /// Past the end: 416.
    Unsatisfiable,
}

/// The one range `header` asks for in `size` bytes (pure).
pub fn parse_range(header: Option<&str>, size: u64) -> Range {
    let Some((a, b)) = header.and_then(|h| h.trim().strip_prefix("bytes=")).and_then(|r| r.split_once('-')) else {
        return Range::Whole;
    };
    let digits = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
    let num = |s: &str| if s.is_empty() { Ok(None) } else { s.parse::<u64>().map(Some) };
    // A list, a non-number, a number too big to read: ignored.
    let (true, true, Ok(a), Ok(b)) = (digits(a), digits(b), num(a), num(b)) else { return Range::Whole };
    match (a, b) {
        (None, None) => Range::Whole,
        (None, Some(n)) if n == 0 || size == 0 => Range::Unsatisfiable,
        (None, Some(n)) => Range::Part(size.saturating_sub(n), size),
        (Some(s), _) if s >= size => Range::Unsatisfiable,
        (Some(s), None) => Range::Part(s, size),
        (Some(s), Some(e)) if e < s => Range::Whole,
        (Some(s), Some(e)) => Range::Part(s, e.saturating_add(1).min(size)),
    }
}

/// The part of an open file a reply sends, read as it goes out.
#[derive(Clone, Debug)]
pub(crate) struct Window {
    pub(crate) file: Arc<File>,
    pub(crate) start: u64,
    pub(crate) len: u64,
    /// Sent gzipped, so of no length known up front.
    pub(crate) gzip: bool,
}

impl PartialEq for Window {
    fn eq(&self, o: &Self) -> bool {
        Arc::ptr_eq(&self.file, &o.file) && (self.start, self.len, self.gzip) == (o.start, o.len, o.gzip)
    }
}

impl Eq for Window {}

fn read_at(f: &File, buf: &mut [u8], at: u64) -> std::io::Result<usize> {
    #[cfg(unix)]
    return std::os::unix::fs::FileExt::read_at(f, buf, at);
    #[cfg(windows)]
    return std::os::windows::fs::FileExt::seek_read(f, buf, at);
}

impl Window {
    /// The bytes, a chunk per step; the file is let go with the last one.
    pub(crate) fn stream(self) -> impl futures_util::Stream<Item = std::io::Result<Vec<u8>>> + Send {
        let end = self.start + self.len;
        futures_util::stream::unfold(Some((self.file, self.start)), move |st| async move {
            let (file, at) = st?;
            let n = CHUNK.min(end - at);
            let f = file.clone();
            let read = tokio::task::spawn_blocking(move || {
                let mut buf = vec![0; n as usize];
                let got = read_at(&f, &mut buf, at)?;
                buf.truncate(got);
                Ok::<_, std::io::Error>(buf)
            })
            .await
            .unwrap_or_else(|e| Err(std::io::Error::other(e)));
            match read {
                Ok(b) if b.is_empty() => None,
                Ok(b) => {
                    let next = at + b.len() as u64;
                    Some((Ok(b), (next < end).then_some((file, next))))
                }
                Err(e) => Some((Err(e), None)),
            }
        })
    }

    /// [`Window::stream`], gzipped if it is to be.
    pub(crate) fn body(self) -> axum::body::Body {
        use futures_util::StreamExt;
        if !self.gzip {
            return axum::body::Body::from_stream(self.stream());
        }
        let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let chunks = Box::pin(self.stream());
        let out = futures_util::stream::unfold((chunks, Some(enc)), |(mut chunks, enc)| async move {
            let mut enc = enc?;
            let next = match chunks.next().await {
                Some(Ok(b)) => std::io::Write::write_all(&mut enc, &b).map(|()| std::mem::take(enc.get_mut())),
                Some(Err(e)) => return Some((Err(e), (chunks, None))),
                None => return Some((enc.finish(), (chunks, None))),
            };
            Some((next, (chunks, Some(enc))))
        });
        axum::body::Body::from_stream(out)
    }

    /// All of it, now (small files, so they can be compressed).
    pub(crate) fn read(&self) -> std::io::Result<Vec<u8>> {
        let mut buf = vec![0; self.len as usize];
        let mut got = 0;
        while got < buf.len() {
            match read_at(&self.file, &mut buf[got..], self.start + got as u64)? {
                0 => break,
                n => got += n,
            }
        }
        buf.truncate(got);
        Ok(buf)
    }
}

/// An open file to serve.
pub(crate) struct Opened {
    pub(crate) file: Arc<File>,
    pub(crate) size: u64,
    /// `W/"<mtime ms>-<size>"`.
    pub(crate) etag: String,
}

/// No file system holds a file here: a NUL, a name over 255 bytes, a path
/// over PATH_MAX (POSIX's; Windows takes longer ones). Pure.
pub fn cannot_exist(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.contains('\0') || (cfg!(not(windows)) && s.len() > 4096) || s.split(['/', '\\']).any(|seg| seg.len() > 255)
}

/// A Windows device name: opens, then fails its stat.
fn device(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let name = name.trim_end_matches(['.', ' ']);
    let numbered =
        |p: &str| name.strip_prefix(p).is_some_and(|n| n.chars().count() == 1 && "0123456789¹²³".contains(n));
    cfg!(windows)
        && (["con", "prn", "aux", "nul", "conin$", "conout$"].contains(&name) || numbered("com") || numbered("lpt"))
}

/// An error that means "no such file": a 404, never a 500 naming the path.
fn missing(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    #[cfg(unix)]
    let looped = e.raw_os_error() == Some(libc::ELOOP);
    #[cfg(windows)]
    let looped = e.raw_os_error() == Some(1921); // ERROR_CANT_RESOLVE_FILENAME
    looped || matches!(e.kind(), NotFound | NotADirectory | IsADirectory | InvalidFilename)
}

/// Open `path` to serve it: `Ok(None)` when there is no file to serve
/// (missing, a directory, a name no file can have).
pub(crate) fn open(path: &Path) -> std::io::Result<Option<Opened>> {
    if cannot_exist(path) {
        return Ok(None);
    }
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if missing(&e) => return Ok(None),
        // Windows refuses to open a folder as a file: still no file.
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && path.is_dir() => return Ok(None),
        Err(e) => return Err(e),
    };
    let meta = match file.metadata() {
        Ok(m) => m,
        Err(_) if device(path) => return Ok(None),
        Err(e) => return Err(e),
    };
    if !meta.is_file() {
        return Ok(None);
    }
    let ms =
        meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis());
    Ok(Some(Opened { file: Arc::new(file), size: meta.len(), etag: format!("W/\"{ms}-{}\"", meta.len()) }))
}

/// What `f` answers a GET/HEAD with `req`'s headers: 304 when it is cached
/// already, else 416, 206 or 200 by `Range`. `headers` go on all but a 416.
/// A weak tag never satisfies `If-Range`: a changed file is sent whole.
pub(crate) fn send(req: &HeaderMap, f: &Opened, headers: Vec<(String, String)>) -> Reply {
    let h = |n: &str| req.get(n).and_then(|v| v.to_str().ok());
    let headers = [headers, vec![("accept-ranges".into(), "bytes".into())]].concat();
    let reply = |status, start, len| Reply {
        status,
        headers: headers.clone(),
        body: Vec::new(),
        window: Some(Window { file: f.file.clone(), start, len, gzip: false }),
    };
    if headers.iter().any(|(k, v)| k == "etag" && etag_matches(h("if-none-match"), v)) {
        return Reply { status: 304, headers, body: Vec::new(), window: None };
    }
    let range = if req.contains_key("if-range") { Range::Whole } else { parse_range(h("range"), f.size) };
    match range {
        Range::Whole => reply(200, 0, f.size),
        Range::Part(s, e) => reply(206, s, e - s).header("content-range", &format!("bytes {s}-{}/{}", e - 1, f.size)),
        Range::Unsatisfiable => Reply::text("Range Not Satisfiable")
            .status(416)
            .header("content-range", &format!("bytes */{}", f.size))
            .header("cache-control", "no-store"),
    }
}

/// The type a file a route serves goes out as, from its name: only kinds a
/// browser shows inertly (images but SVG, video, audio, PDF, plain text);
/// anything else is `application/octet-stream`, which no browser renders.
pub fn inert_type(name: &str) -> &'static str {
    match super::content_type(name) {
        Some(
            t @ ("image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif" | "image/x-icon"
            | "application/pdf" | "text/plain"),
        ) => t,
        Some(t) if t.starts_with("video/") || t.starts_with("audio/") => t,
        _ => "application/octet-stream",
    }
}

/// Serve the file at `path`, a path the app chose (never a raw request
/// path: keeping it inside a folder is the caller's job), as a blob is:
/// GET/HEAD only (405), Range, a weak mtime+size `ETag` (304), `nosniff`,
/// `Content-Security-Policy: sandbox`, `no-transform` (a resumed download
/// never glues raw bytes onto a compressed start). Typed by
/// an inert type unless `content_type` says. Missing or a folder: 404.
///
/// ```ignore
/// Route::get("/media/:name", move |r| {
///     let dir = dir.clone();
///     async move {
///         let name = r.param("name").unwrap_or_default();
///         // basename only: a param may hold `/` or `..`
///         let f = dir.join(Path::new(name).file_name().unwrap_or_default());
///         aio::http::serve_file(&r, f, None).await
///     }
/// })
/// ```
pub async fn serve_file(req: &super::Req, path: impl AsRef<Path>, content_type: Option<&str>) -> Reply {
    if req.method != "GET" && req.method != "HEAD" {
        return Reply::text("Method Not Allowed").status(405).header("allow", "GET, HEAD");
    }
    let path = path.as_ref().to_path_buf();
    let ct = content_type.map_or_else(|| inert_type(&super::path_name(&path)).to_string(), String::from);
    let opened = tokio::task::spawn_blocking(move || open(&path)).await;
    match opened {
        Ok(Ok(Some(f))) => {
            let headers = [
                ("content-type", ct.as_str()),
                ("etag", &f.etag),
                ("cache-control", "no-cache, no-transform"),
                ("content-security-policy", "sandbox"),
                ("x-content-type-options", "nosniff"),
            ];
            send(&req.headers, &f, headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
        }
        Ok(Ok(None)) => Reply::text("Not Found").status(404),
        Ok(Err(e)) => super::broken(&format!("a file can't be served: {}", e.kind())),
        Err(e) => super::broken(&format!("a file can't be served: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_follow_rfc_9110_as_aio() {
        use Range::*;
        let table: &[(Option<&str>, Range)] = &[
            (Some("bytes=0-4"), Part(0, 5)),
            (Some("bytes=15-"), Part(15, 20)),
            (Some("bytes=-3"), Part(17, 20)),
            (Some("bytes=18-500"), Part(18, 20)),
            (Some("bytes=-500"), Part(0, 20)),
            (Some("bytes=19-19"), Part(19, 20)),
            (Some(" bytes=0-0 "), Part(0, 1)),
            (Some("bytes=5-2"), Whole),
            (Some("bytes=0-1,4-5"), Whole),
            (Some("chunks=0-1"), Whole),
            (Some("bytes=-"), Whole),
            (Some("bytes=a-1"), Whole),
            (Some("bytes=99999999999999999999999-"), Whole),
            (None, Whole),
            (Some("bytes=20-"), Unsatisfiable),
            (Some("bytes=-0"), Unsatisfiable),
            (Some("bytes=99-100"), Unsatisfiable),
        ];
        for (h, want) in table {
            assert_eq!(&parse_range(*h, 20), want, "{h:?}");
        }
        assert_eq!(parse_range(Some("bytes=-5"), 0), Unsatisfiable);
    }

    #[test]
    fn only_inert_types_are_inferred() {
        for (name, t) in [
            ("clip.mp4", "video/mp4"),
            ("a.PNG", "image/png"),
            ("song.mp3", "audio/mpeg"),
            ("doc.pdf", "application/pdf"),
            ("n.txt", "text/plain"),
            ("evil.html", "application/octet-stream"),
            ("evil.svg", "application/octet-stream"),
            ("x.js", "application/octet-stream"),
            ("noext", "application/octet-stream"),
        ] {
            assert_eq!(inert_type(name), t, "{name}");
        }
    }

    #[tokio::test]
    async fn a_streamed_file_is_let_go_with_its_last_byte() {
        use futures_util::StreamExt;
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.bin");
        std::fs::write(&p, vec![7u8; (CHUNK * 2 + 5) as usize]).unwrap();
        let f = open(&p).unwrap().unwrap();
        let mut s = std::pin::pin!(Window { file: f.file.clone(), start: 3, len: CHUNK * 2, gzip: false }.stream());
        let mut got = 0;
        while got < CHUNK * 2 {
            got += s.next().await.unwrap().unwrap().len() as u64;
        }
        // A client that has every byte asks no more: nothing holds the file.
        assert_eq!((got, Arc::strong_count(&f.file)), (CHUNK * 2, 1));
        assert!(s.next().await.is_none());
        assert_eq!(open(t.path()).unwrap().map(|o| o.size), None, "a folder is no file");
        assert_eq!(open(&p.join("x")).unwrap().map(|o| o.size), None, "a path through a file");
        #[cfg(unix)]
        {
            let l = t.path().join("loop");
            std::os::unix::fs::symlink(t.path().join("loop2"), &l).unwrap();
            std::os::unix::fs::symlink(&l, t.path().join("loop2")).unwrap();
            assert_eq!(open(&l).unwrap().map(|o| o.size), None, "a symlink cycle");
        }
    }

    #[test]
    fn a_name_no_file_can_have_cannot_exist() {
        assert!(cannot_exist(Path::new(&format!("/tmp/{}", "x".repeat(300)))));
        assert!(cannot_exist(Path::new("/tmp/a\0b")));
        assert!(!cannot_exist(Path::new("/tmp/aux.txt")));
        #[cfg(not(windows))]
        assert!(cannot_exist(Path::new(&"/a".repeat(2100))));
    }
}

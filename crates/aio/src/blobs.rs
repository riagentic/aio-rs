//! Blobs (aio's `app.blobs`): a content-addressed byte store in the app's
//! `files` tier (`<data>/files/blobs`, inside the backup unit). Bytes never
//! enter a cell, the event log or the wire: a cell holds a blob's id, and
//! the bytes travel over HTTP (`/__aio/blobs/<id>`, Range, cached for good:
//! the id is the content's SHA-256) or as a plain file.
//!
//! - `<id>`: the bytes; `<id>.json`: `{name}`, the first name it was put
//!   with (the name never changes the id: same bytes, one blob).
//! - [`Blobs::put`] streams: the bytes are hashed and written to a temp file
//!   as they come, synced, then renamed to their hash, so a blob is whole
//!   under its id or absent. A crash leaves a `.tmp-*`, swept by a later put
//!   once it is an hour old.
//! - Absent is `None` or `false`; a store that can't be read is an error,
//!   never an empty answer.
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Where blob bytes are served: `/__aio/blobs/<id>`.
pub const URL_PREFIX: &str = "/__aio/blobs/";

/// What the store knows of one blob.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blob {
    /// The SHA-256 of its bytes, lowercase hex: its address.
    pub id: String,
    pub size: u64,
    /// The name it was first put with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A blob id: 64 lowercase hex characters. One decider for the store and
/// the route, so no crafted id ever becomes a path.
pub fn id_ok(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The path blob `id` is served at.
pub fn url(id: &str) -> String {
    format!("{URL_PREFIX}{id}")
}

fn bad_id(id: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("blobs: {id:?} is not a blob id (the 64 hex characters of a SHA-256, as put returns)"),
    )
}

/// How old a `.tmp-*` must be to be swept.
const STALE: std::time::Duration = std::time::Duration::from_secs(3600);

/// One app's blobs.
#[derive(Clone, Debug)]
pub struct Blobs {
    dir: PathBuf,
}

impl Blobs {
    /// The store in `dir` (made on the first put).
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// App `dirs`' store: `<data>/files/blobs`.
    pub fn of(dirs: &crate::paths::AppDirs) -> Self {
        Self::at(dirs.files().join("blobs"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file holding blob `id`'s bytes, if it is here.
    pub fn path(&self, id: &str) -> io::Result<Option<PathBuf>> {
        if !id_ok(id) {
            return Err(bad_id(id));
        }
        let p = self.dir.join(id);
        match std::fs::metadata(&p) {
            Ok(m) if m.is_file() => Ok(Some(p)),
            Ok(_) => Ok(None),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(self.unreadable(id, e)),
        }
    }

    fn unreadable(&self, what: &str, e: io::Error) -> io::Error {
        io::Error::new(e.kind(), format!("blobs: can't read {what} in {}: {e}", self.dir.display()))
    }

    /// Store the bytes `r` gives, hashed and written as they are read, never
    /// held whole. The same bytes give the same id and one file; `name` is
    /// kept only by the first put.
    pub fn put(&self, mut r: impl Read, name: Option<&str>) -> io::Result<Blob> {
        crate::paths::private_dir(&self.dir)?;
        self.sweep();
        let tmp = self.dir.join(format!(".tmp-{}", crate::uuid_v4()));
        let written = (|| {
            let mut f = std::fs::File::create_new(&tmp)?;
            let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
            let (mut buf, mut size) = (vec![0u8; 64 << 10], 0u64);
            loop {
                let n = match r.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                };
                hash.update(&buf[..n]);
                f.write_all(&buf[..n])?;
                size += n as u64;
            }
            f.sync_all()?;
            let id: String = hash.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect();
            Ok((id, size))
        })();
        let (id, size) = match written {
            Ok(w) => w,
            Err(e) => {
                _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        };
        let at = self.dir.join(&id);
        if at.is_file() {
            _ = std::fs::remove_file(&tmp);
        } else {
            crate::paths::rename_over(&tmp, &at)?;
            sync_dir(&self.dir);
        }
        let name = match name {
            Some(n) => self.first_name(&id, n)?,
            None => self.name(&id)?,
        };
        Ok(Blob { id, size, name })
    }

    /// Record `name` for `id` unless one is there: the first wins, also
    /// between puts racing (create-new is atomic).
    fn first_name(&self, id: &str, name: &str) -> io::Result<Option<String>> {
        let meta = serde_json::json!({ "name": name }).to_string();
        let at = self.dir.join(format!("{id}.json"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&at) {
            Ok(mut f) => {
                f.write_all(meta.as_bytes())?;
                Ok(Some(name.into()))
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => self.name(id),
            Err(e) => Err(e),
        }
    }

    fn name(&self, id: &str) -> io::Result<Option<String>> {
        match std::fs::read(self.dir.join(format!("{id}.json"))) {
            Ok(b) => Ok(serde_json::from_slice::<serde_json::Value>(&b)
                .ok()
                .and_then(|v| v["name"].as_str().map(String::from))
                .or_else(|| {
                    eprintln!(
                        "[aio] blobs: the name of blob {id} is unreadable: its bytes are whole, its name is lost"
                    );
                    None
                })),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(self.unreadable(&format!("the name of blob {id}"), e)),
        }
    }

    /// Blob `id`, or `None` when it isn't here.
    pub fn info(&self, id: &str) -> io::Result<Option<Blob>> {
        let Some(p) = self.path(id)? else { return Ok(None) };
        let size = std::fs::metadata(&p).map_err(|e| self.unreadable(id, e))?.len();
        Ok(Some(Blob { id: id.into(), size, name: self.name(id)? }))
    }

    /// Blob `id`'s bytes `start..end` (end exclusive, clamped to its size).
    /// A blob that isn't here is an error naming it.
    pub fn read(&self, id: &str, start: u64, end: u64) -> io::Result<Vec<u8>> {
        use std::io::Seek;
        let p = self.path(id)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("blobs: no blob {id} in {}", self.dir.display()))
        })?;
        let mut f = std::fs::File::open(p)?;
        f.seek(io::SeekFrom::Start(start))?;
        let mut out = Vec::new();
        f.take(end.saturating_sub(start)).read_to_end(&mut out)?;
        Ok(out)
    }

    /// Remove blob `id` and its name: `false` when it wasn't here. Every put
    /// of these bytes got this id: remove it only when nothing names it.
    pub fn delete(&self, id: &str) -> io::Result<bool> {
        let Some(p) = self.path(id)? else { return Ok(false) };
        std::fs::remove_file(p)?;
        match std::fs::remove_file(self.dir.join(format!("{id}.json"))) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        Ok(true)
    }

    /// Every blob, by id.
    pub fn list(&self) -> io::Result<Vec<Blob>> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(self.unreadable("the store", e)),
        };
        let mut out = vec![];
        for e in entries {
            let name = e.map_err(|e| self.unreadable("the store", e))?.file_name();
            if let Some(b) = name.to_str().filter(|n| id_ok(n)).map(|id| self.info(id)).transpose()?.flatten() {
                out.push(b);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// Remove `.tmp-*` an hour old (a put that crashed); best effort.
    fn sweep(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return };
        for e in entries.flatten() {
            let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok());
            if e.file_name().to_string_lossy().starts_with(".tmp-") && old.is_some_and(|age| age > STALE) {
                _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Make a rename in `dir` durable (Unix: fsync the dir; Windows needs none).
fn sync_dir(dir: &Path) {
    if cfg!(unix)
        && let Ok(d) = std::fs::File::open(dir)
    {
        _ = d.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(b: &[u8]) -> String {
        ring::digest::digest(&ring::digest::SHA256, b).as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn a_put_is_addressed_by_its_bytes_and_reads_back_in_windows() {
        let d = tempfile::tempdir().unwrap();
        let s = Blobs::at(d.path().join("blobs"));
        assert_eq!(s.list().unwrap(), [], "an empty store is empty, not broken");
        let fox = b"The quick brown fox jumps over the lazy dog";
        let b = s.put(&fox[..], Some("fox.txt")).unwrap();
        assert_eq!((b.id.as_str(), b.size, b.name.as_deref()), (sha(fox).as_str(), 43, Some("fox.txt")));
        assert_eq!(s.read(&b.id, 0, u64::MAX).unwrap(), fox);
        assert_eq!(s.read(&b.id, 4, 9).unwrap(), b"quick", "end exclusive");
        assert_eq!(s.read(&b.id, 40, 99).unwrap(), b"dog", "past the end: clamped");
        assert_eq!(s.read(&b.id, 50, 60).unwrap(), b"");
        // Same bytes: same id, one file, the first name kept.
        let again = s.put(&fox[..], Some("other.txt")).unwrap();
        assert_eq!(again, b);
        assert_eq!(s.put(&fox[..], None).unwrap().name.as_deref(), Some("fox.txt"));
        let files: Vec<_> = std::fs::read_dir(s.dir()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(files.len(), 2, "{files:?}");
        assert_eq!(url(&b.id), format!("/__aio/blobs/{}", b.id));
        // Delete: bytes and name; list follows.
        let c = s.put(&b"x"[..], None).unwrap();
        let mut both = vec![c.clone(), b.clone()];
        both.sort_by(|x, y| x.id.cmp(&y.id));
        assert_eq!(s.list().unwrap(), both);
        assert!(s.delete(&b.id).unwrap());
        assert!(!s.delete(&b.id).unwrap(), "nothing there");
        assert_eq!((s.info(&b.id).unwrap(), s.list().unwrap()), (None, vec![c]));
        assert!(!s.dir().join(format!("{}.json", b.id)).exists());
    }

    #[test]
    fn a_big_put_streams_and_bad_ids_are_refused_by_name() {
        let d = tempfile::tempdir().unwrap();
        let s = Blobs::at(d.path());
        // 32 MiB read 64 KiB at a time, never whole.
        struct Pattern(u64);
        impl Read for Pattern {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = buf.len().min(usize::try_from(self.0).unwrap_or(usize::MAX));
                buf[..n].iter_mut().enumerate().for_each(|(i, b)| *b = (i % 251) as u8);
                self.0 -= n as u64;
                Ok(n)
            }
        }
        let b = s.put(Pattern(32 << 20), None).unwrap();
        assert_eq!(b.size, 32 << 20);
        let mut whole = ring::digest::Context::new(&ring::digest::SHA256);
        let mut p = Pattern(32 << 20);
        let mut buf = vec![0u8; 64 << 10];
        while let Ok(n @ 1..) = p.read(&mut buf) {
            whole.update(&buf[..n]);
        }
        assert_eq!(b.id, whole.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect::<String>());
        assert!(
            std::fs::read_dir(d.path()).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tmp-"))
        );
        for bad in ["not-an-id", "../etc/passwd", &"A".repeat(64), &"0".repeat(63)] {
            assert!(s.info(bad).unwrap_err().to_string().contains("is not a blob id"), "{bad}");
            assert!(!id_ok(bad));
        }
        let missing = "0".repeat(64);
        assert_eq!(s.info(&missing).unwrap(), None);
        assert!(s.read(&missing, 0, 1).unwrap_err().to_string().contains(&format!("no blob {missing}")));
    }

    #[test]
    fn a_crashed_put_is_swept_once_stale_and_a_failed_read_leaves_nothing() {
        let d = tempfile::tempdir().unwrap();
        let s = Blobs::at(d.path());
        let (stale, fresh) = (d.path().join(".tmp-old"), d.path().join(".tmp-new"));
        std::fs::write(&stale, b"half").unwrap();
        std::fs::write(&fresh, b"half").unwrap();
        let hour_ago = std::time::SystemTime::now() - STALE - std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(&stale).unwrap().set_modified(hour_ago).unwrap();
        s.put(&b"a"[..], None).unwrap();
        assert!(!stale.exists() && fresh.exists(), "only the stale one");
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("the upload dropped"))
            }
        }
        assert!(s.put(Broken, Some("x")).is_err());
        let left: Vec<_> = std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 2, "the put and the fresh tmp: {left:?}");
    }
}

//! Delta updates (aio's `updates-tree.ts`, `updates-patch.ts`): a release made
//! from the version installed here, file by file, instead of downloaded
//! whole. Two builds a commit apart differ by kilobytes in a binary of
//! megabytes, while their packed bundles differ from the first changed byte
//! on (deflate), so a patch is per file of the unpacked install.
//!
//! A patch is the new release's file list (path, size, SHA-256, exec bit)
//! and, per file, how to make it: the same file as installed, a zstd frame
//! against the installed file at that path (`zstd --patch-from`), or a frame
//! of its own. Its SHA-256 is in the release's signed patch list
//! (`aio::update::Patches`), so its file list is as trusted as the release.
//! Every file made is checked against it; any miss fails the patch, and the
//! caller downloads the whole release instead.
use aio::sha256::Sha256;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{self, Read};
use std::path::Path;

const MAGIC: &[u8] = b"aio-tree-patch 1\n";
/// zstd's level for patches: they are made once and fetched many times.
const LEVEL: i32 = 19;
/// The furthest a match reaches back (2^30): a base file and the new one fit.
const WINDOW_LOG: u32 = 30;

/// How one file of the new release is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Op {
    /// The installed file, unchanged.
    Same,
    /// A frame against the installed file (`base` its SHA-256).
    Patch,
    /// A frame of its own.
    New,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    path: String,
    size: u64,
    sha256: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    exec: bool,
    op: Op,
    /// The bytes of its frame (`patch`, `new`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    len: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base: Option<String>,
}

/// The patch's first line after the magic: the new release's files, and
/// its folders that hold none.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    files: Vec<File>,
    dirs: Vec<String>,
}

fn sha256(b: &[u8]) -> String {
    aio::sha256::hex(&Sha256::digest(b))
}

/// The files (relative `/` paths, sorted) and empty folders under `root`.
/// A link is refused: a patch makes files and folders only.
fn tree(root: &Path) -> io::Result<(Vec<String>, Vec<String>)> {
    let (mut files, mut dirs) = (vec![], vec![]);
    let mut todo = vec![String::new()];
    while let Some(rel) = todo.pop() {
        let mut empty = true;
        for e in std::fs::read_dir(root.join(&rel))? {
            let e = e?;
            empty = false;
            let name = e.file_name().into_string().map_err(|n| io::Error::other(format!("{n:?}: not UTF-8")))?;
            let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            let t = e.file_type()?;
            if t.is_symlink() {
                return Err(io::Error::other(format!("{path} is a link: a patch makes files and folders only")));
            }
            if t.is_dir() { todo.push(path) } else { files.push(path) }
        }
        if empty && !rel.is_empty() {
            dirs.push(rel);
        }
    }
    files.sort();
    dirs.sort();
    Ok((files, dirs))
}

#[cfg(unix)]
fn is_exec(p: &Path) -> io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    Ok(std::fs::metadata(p)?.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_exec(_: &Path) -> io::Result<bool> {
    Ok(false)
}

fn compress(new: &[u8], base: Option<&[u8]>) -> io::Result<Vec<u8>> {
    let mut e = match base {
        Some(b) => zstd::stream::write::Encoder::with_ref_prefix(vec![], LEVEL, b)?,
        None => zstd::stream::write::Encoder::new(vec![], LEVEL)?,
    };
    if base.is_some() {
        e.window_log(WINDOW_LOG)?;
        e.long_distance_matching(true)?;
    }
    io::Write::write_all(&mut e, new)?;
    e.finish()
}

/// The patch that makes the release unpacked at `new` from the one at `old`.
pub fn make(old: &Path, new: &Path) -> io::Result<Vec<u8>> {
    let (paths, dirs) = tree(new)?;
    let mut header = Header { files: vec![], dirs };
    let mut frames = vec![];
    for path in paths {
        let bytes = std::fs::read(new.join(&path))?;
        let mut f = File {
            sha256: sha256(&bytes),
            size: bytes.len() as u64,
            exec: is_exec(&new.join(&path))?,
            op: Op::New,
            len: None,
            base: None,
            path,
        };
        let was = std::fs::read(old.join(&f.path)).ok();
        let frame = match was {
            Some(b) if b == bytes => None,
            Some(b) => {
                let (patch, own) = (compress(&bytes, Some(&b))?, compress(&bytes, None)?);
                // A frame of its own when that is smaller (a file rewritten whole).
                if patch.len() < own.len() {
                    (f.op, f.base) = (Op::Patch, Some(sha256(&b)));
                    Some(patch)
                } else {
                    Some(own)
                }
            }
            None => Some(compress(&bytes, None)?),
        };
        match frame {
            None => f.op = Op::Same,
            Some(frame) => {
                f.len = Some(frame.len() as u64);
                frames.push(frame);
            }
        }
        header.files.push(f);
    }
    let mut out = MAGIC.to_vec();
    out.extend(aio::serde_json::to_vec(&header)?);
    out.push(b'\n');
    frames.iter().for_each(|f| out.extend(f));
    Ok(out)
}

/// Why `p` can't name a file inside the install, or `None`: relative, `/`
/// separated, no `.`, `..` or empty part, no `\` or `:` (a Windows drive or
/// stream).
fn bad_path(p: &str) -> Option<String> {
    let bad = p.is_empty()
        || p.starts_with('/')
        || p.contains(['\\', ':', '\0'])
        || p.split('/').any(|s| s.is_empty() || s == "." || s == "..");
    bad.then(|| format!("the path {p:?}"))
}

/// The header and frames of a patch, its file list checked: paths inside
/// the install, each once, frames that add up to the rest of the patch.
fn parse(patch: &[u8]) -> io::Result<(Header, &[u8])> {
    let bad = |why: String| io::Error::new(io::ErrorKind::InvalidData, format!("the patch is refused: {why}"));
    let rest = patch.strip_prefix(MAGIC).ok_or_else(|| bad("not a tree patch".into()))?;
    let end = rest.iter().position(|&b| b == b'\n').ok_or_else(|| bad("no file list".into()))?;
    let h: Header = aio::serde_json::from_slice(&rest[..end]).map_err(|e| bad(format!("its file list: {e}")))?;
    let mut seen = BTreeSet::new();
    for p in h.files.iter().map(|f| &f.path).chain(&h.dirs) {
        if let Some(why) = bad_path(p) {
            return Err(bad(why));
        }
        if !seen.insert(p.to_lowercase()) {
            return Err(bad(format!("it names {p:?} twice")));
        }
    }
    let frames = &rest[end + 1..];
    let total = h.files.iter().try_fold(0u64, |n, f| match (f.op, f.len, &f.base) {
        (Op::Same, None, None) | (Op::New, Some(_), None) | (Op::Patch, Some(_), Some(_)) => {
            n.checked_add(f.len.unwrap_or(0))
        }
        _ => None,
    });
    if total != Some(frames.len() as u64) {
        return Err(bad("its frames don't match its file list".into()));
    }
    Ok((h, frames))
}

fn decompress(frame: &[u8], base: Option<&[u8]>, size: u64) -> io::Result<Vec<u8>> {
    let mut d = match base {
        Some(b) => zstd::stream::read::Decoder::with_ref_prefix(frame, b)?,
        None => zstd::stream::read::Decoder::with_buffer(frame)?,
    };
    d.window_log_max(WINDOW_LOG)?;
    let mut out = Vec::with_capacity(size as usize);
    d.take(size + 1).read_to_end(&mut out)?;
    Ok(out)
}

/// Make the release in `into` (empty) from the version installed at `from`
/// and `patch`. An error names the first file that could not be made as
/// the list says; `into` is then the caller's to remove.
pub fn apply(patch: &[u8], from: &Path, into: &Path) -> io::Result<()> {
    let (h, mut frames) = parse(patch)?;
    for f in &h.files {
        let miss = |why: &str| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {why}", f.path));
        let base = match f.op {
            Op::New => None,
            Op::Same | Op::Patch => Some(std::fs::read(from.join(&f.path)).map_err(|e| miss(&e.to_string()))?),
        };
        let want = if f.op == Op::Same { &f.sha256 } else { f.base.as_ref().unwrap_or(&f.sha256) };
        if base.as_deref().is_some_and(|b| sha256(b) != *want) {
            return Err(miss("the installed file is not the one the patch starts from"));
        }
        let bytes = match (f.op, base) {
            (Op::Same, Some(b)) => b,
            (op, base) => {
                let (frame, rest) = frames.split_at(f.len.unwrap_or(0) as usize);
                frames = rest;
                let base = base.filter(|_| op == Op::Patch);
                decompress(frame, base.as_deref(), f.size).map_err(|e| miss(&e.to_string()))?
            }
        };
        if bytes.len() as u64 != f.size || sha256(&bytes) != f.sha256 {
            return Err(miss("the patch made other bytes than the release's"));
        }
        let to = into.join(&f.path);
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&to, &bytes)?;
        #[cfg(unix)]
        if f.exec {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    h.dirs.iter().try_for_each(|d| std::fs::create_dir_all(into.join(d)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A release unpacked: `files` (path, bytes).
    fn release(dir: &Path, files: &[(&str, &[u8])]) {
        for (p, b) in files {
            let to = dir.join(p);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::write(to, b).unwrap();
        }
    }

    /// 400 kB that don't compress, as a binary's code nearly doesn't.
    fn binary() -> Vec<u8> {
        (0..400_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect()
    }

    #[test]
    fn a_patch_makes_the_new_release_from_the_old_one_file_by_file() {
        let t = tempfile::tempdir().unwrap();
        let (old, new, made) = (t.path().join("old"), t.path().join("new"), t.path().join("made"));
        let mut bin = binary();
        release(&old, &[("notes", &bin), ("notes.png", b"icon"), ("gone.txt", b"bye"), ("rewritten", b"aaaa")]);
        // One line of code changed: a few bytes of the binary.
        bin[1000..1010].copy_from_slice(b"new string");
        release(&new, &[("notes", &bin), ("notes.png", b"icon"), ("docs/README", b"hi"), ("rewritten", b"zz")]);
        std::fs::create_dir_all(new.join("cache")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(new.join("notes"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let p = make(&old, &new).unwrap();
        assert!(p.len() < 2_000, "a small change is a small patch: {} bytes", p.len());
        let (h, _) = parse(&p).unwrap();
        let ops: Vec<_> = h.files.iter().map(|f| (f.path.as_str(), f.op)).collect();
        // A file rewritten whole is a frame either way: whichever is smaller.
        assert_eq!(ops[..3], [("docs/README", Op::New), ("notes", Op::Patch), ("notes.png", Op::Same)]);
        assert_eq!(ops[3].0, "rewritten");
        assert_ne!(ops[3].1, Op::Same);
        assert_eq!(h.dirs, ["cache"]);
        apply(&p, &old, &made).unwrap();
        assert_eq!(tree(&made).unwrap(), tree(&new).unwrap(), "the same files, and nothing of the old");
        for f in ["notes", "notes.png", "docs/README", "rewritten"] {
            assert_eq!(std::fs::read(made.join(f)).unwrap(), std::fs::read(new.join(f)).unwrap(), "{f}");
        }
        #[cfg(unix)]
        assert!(is_exec(&made.join("notes")).unwrap() && !is_exec(&made.join("notes.png")).unwrap());
    }

    #[test]
    fn an_installed_file_changed_since_fails_the_patch() {
        let t = tempfile::tempdir().unwrap();
        let (old, new) = (t.path().join("old"), t.path().join("new"));
        let mut bin = binary();
        release(&old, &[("notes", &bin), ("notes.png", b"icon")]);
        bin[5..9].copy_from_slice(b"v1.1");
        release(&new, &[("notes", &bin), ("notes.png", b"icon")]);
        let p = make(&old, &new).unwrap();
        for (file, edit) in [("notes", b"x".as_slice()), ("notes.png", b"ICON")] {
            let here = t.path().join(format!("here-{file}"));
            release(&here, &[("notes", &binary()), ("notes.png", b"icon")]);
            std::fs::write(here.join(file), edit).unwrap();
            let e = apply(&p, &here, &t.path().join(format!("made-{file}"))).unwrap_err().to_string();
            assert_eq!(e, format!("{file}: the installed file is not the one the patch starts from"));
        }
        // One gone is a miss too.
        let gone = t.path().join("gone");
        release(&gone, &[("notes.png", b"icon")]);
        assert!(apply(&p, &gone, &t.path().join("made-gone")).unwrap_err().to_string().starts_with("notes: "));
    }

    #[test]
    fn a_list_that_names_a_file_outside_the_install_or_one_twice_is_refused() {
        let file = |path: &str| File {
            path: path.into(),
            size: 0,
            sha256: sha256(b""),
            exec: false,
            op: Op::Same,
            len: None,
            base: None,
        };
        let patch = |h: &Header, frames: &[u8]| {
            let mut p = MAGIC.to_vec();
            p.extend(aio::serde_json::to_vec(h).unwrap());
            p.push(b'\n');
            p.extend(frames);
            p
        };
        for path in ["../x", "/etc/passwd", "a//b", "a/./b", "C:x", "a\\b", ""] {
            let p = patch(&Header { files: vec![file(path)], dirs: vec![] }, b"");
            let e = parse(&p).unwrap_err().to_string();
            assert!(e.starts_with("the patch is refused: the path"), "{path}: {e}");
        }
        let twice = patch(&Header { files: vec![file("a"), file("A")], dirs: vec![] }, b"");
        assert!(parse(&twice).unwrap_err().to_string().contains("twice"));
        let dir_too = patch(&Header { files: vec![file("a")], dirs: vec!["a".into()] }, b"");
        assert!(parse(&dir_too).unwrap_err().to_string().contains("twice"));
        // Frames that don't add up to the list, or a list that doesn't say how.
        let extra = patch(&Header { files: vec![file("a")], dirs: vec![] }, b"junk");
        assert!(parse(&extra).unwrap_err().to_string().contains("frames don't match"));
        let mut odd = file("a");
        odd.op = Op::Patch;
        odd.len = Some(0);
        assert!(parse(&patch(&Header { files: vec![odd], dirs: vec![] }, b"")).is_err());
        assert!(parse(b"something else").unwrap_err().to_string().contains("not a tree patch"));
    }

    #[test]
    fn a_frame_that_makes_other_bytes_is_said() {
        let t = tempfile::tempdir().unwrap();
        let (old, new) = (t.path().join("old"), t.path().join("new"));
        release(&old, &[("a", b"one")]);
        release(&new, &[("a", b"two"), ("b", b"three")]);
        let mut p = make(&old, &new).unwrap();
        // The list says another digest for "b" than its frame makes.
        let at = p.windows(64).position(|w| w == sha256(b"three").as_bytes()).unwrap();
        p[at..at + 64].copy_from_slice(sha256(b"thr33").as_bytes());
        let e = apply(&p, &old, &t.path().join("made")).unwrap_err().to_string();
        assert_eq!(e, "b: the patch made other bytes than the release's");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_no_part_of_a_patch() {
        let t = tempfile::tempdir().unwrap();
        release(&t.path().join("new"), &[("a", b"1")]);
        std::os::unix::fs::symlink("a", t.path().join("new/b")).unwrap();
        let e = make(&t.path().join("old"), &t.path().join("new")).unwrap_err().to_string();
        assert!(e.contains("b is a link"), "{e}");
    }
}

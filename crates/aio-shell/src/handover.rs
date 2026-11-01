//! Files from a door that hands over descriptors, never paths (Android's
//! system picker: a document the app may open, wherever it lives). A picked
//! file is copied in and given as a path; a file to save is a path whose
//! bytes reach the document when its writer closes it.
use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `name` as one file name: no separator or control character, never empty,
/// `.` or `..` (any app's document provider names it).
pub fn leaf(name: &str) -> String {
    let s: String = name.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).collect();
    match s.trim() {
        "" | "." | ".." => "file".into(),
        s => s.into(),
    }
}

/// A new folder under `dir` for one answer.
fn fresh(dir: &Path) -> io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let d = dir.join(format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

/// Each picked file (its name, its descriptor) copied into its own folder
/// under `dir`, name kept: their paths, in order.
pub fn take_in(dir: &Path, files: Vec<(String, OwnedFd)>) -> io::Result<Vec<String>> {
    let into = fresh(dir)?;
    let mut paths = Vec::with_capacity(files.len());
    for (i, (name, fd)) in files.into_iter().enumerate() {
        let to = into.join(i.to_string()).join(leaf(&name));
        std::fs::create_dir_all(to.parent().unwrap_or(&into))?;
        io::copy(&mut File::from(fd), &mut File::create(&to)?)?;
        paths.push(to.to_string_lossy().into_owned());
    }
    Ok(paths)
}

/// A path under `dir` named `name` to write once (`std::fs::write`): what is
/// written there reaches `to` when the writer closes it. Nobody writing
/// within `wait`, `to` stays empty and the path goes.
pub fn save_through(dir: &Path, name: &str, to: OwnedFd, wait: Duration) -> io::Result<String> {
    let into = fresh(dir)?;
    let path = into.join(leaf(name));
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(io::Error::other)?;
    // SAFETY: a valid C string; mkfifo reads it only.
    if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (fifo, gone) = (path.clone(), into.clone());
    std::thread::spawn(move || {
        // Opening blocks until a writer opens it; its close is the end.
        let copied = File::open(&fifo).and_then(|mut from| io::copy(&mut from, &mut File::from(to)));
        _ = std::fs::remove_dir_all(&gone);
        if let Err(e) = copied {
            eprintln!("[aio-shell] the saved file did not reach its document: {e}");
        }
    });
    let late = path.clone();
    std::thread::spawn(move || {
        std::thread::sleep(wait);
        // Still waiting for a writer: this open ends it (nothing written).
        use std::os::unix::fs::OpenOptionsExt;
        if std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&late).is_ok() {
            eprintln!("[aio-shell] nothing was written to {} in {wait:?}: the document stays empty", late.display());
        }
    });
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fd(f: File) -> OwnedFd {
        f.into()
    }

    #[test]
    fn a_name_from_any_provider_is_one_file_name() {
        assert_eq!(leaf("notes.md"), "notes.md");
        assert_eq!(leaf("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(leaf("a\\b\n.csv"), "a_b_.csv");
        for none in ["", " ", ".", ".."] {
            assert_eq!(leaf(none), "file", "{none:?}");
        }
    }

    #[test]
    fn picked_files_are_copied_in_each_under_its_own_name() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        std::fs::create_dir(&src).unwrap();
        let (a, b) = (src.join("a"), src.join("b"));
        std::fs::write(&a, "first").unwrap();
        std::fs::write(&b, "second").unwrap();
        let open = |p: &Path| fd(File::open(p).unwrap());
        let paths =
            take_in(&d.path().join("picked"), vec![("x.txt".into(), open(&a)), ("x.txt".into(), open(&b))]).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().all(|p| p.ends_with("/x.txt")), "{paths:?}");
        let read: Vec<String> = paths.iter().map(|p| std::fs::read_to_string(p).unwrap()).collect();
        assert_eq!(read, ["first", "second"], "the same name twice: both kept, in order");
        let bad = take_in(&d.path().join("picked"), vec![("../up".into(), open(&a))]).unwrap();
        assert!(Path::new(&bad[0]).starts_with(d.path().join("picked")), "never outside: {bad:?}");
    }

    #[test]
    fn what_is_written_to_the_save_path_reaches_the_document_when_closed() {
        let d = tempfile::tempdir().unwrap();
        let doc = d.path().join("doc.csv");
        let path = save_through(
            &d.path().join("saving"),
            "history.csv",
            fd(File::create(&doc).unwrap()),
            Duration::from_secs(60),
        )
        .unwrap();
        assert!(path.ends_with("/history.csv"), "{path}");
        std::fs::write(&path, "a,b\n1,2\n").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::fs::read_to_string(&doc).unwrap() != "a,b\n1,2\n" {
            assert!(std::time::Instant::now() < deadline, "the bytes never arrived");
            std::thread::sleep(Duration::from_millis(10));
        }
        while Path::new(&path).exists() {
            assert!(std::time::Instant::now() < deadline, "the save path stayed");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_save_nobody_writes_ends_empty_and_its_path_goes() {
        let d = tempfile::tempdir().unwrap();
        let doc = d.path().join("doc");
        let path =
            save_through(&d.path().join("saving"), "x", fd(File::create(&doc).unwrap()), Duration::from_millis(50))
                .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while Path::new(&path).exists() {
            assert!(std::time::Instant::now() < deadline, "the reader never let go");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(std::fs::read(&doc).unwrap(), b"");
        _ = std::fs::write(&path, "late");
        assert_eq!(std::fs::read(&doc).unwrap(), b"", "a late write never blocks, and never reaches it");
    }
}

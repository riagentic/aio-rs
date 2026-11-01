//! What the runtime writes on stderr, sorted (aio electron-renderer-log):
//! the page's and the shell's own lines (`[page]`, `[aio-shell]`) and
//! anything else Electron says go to the app's log; only Chromium's known,
//! harmless chatter is dropped, in its exact shapes, and said once per kind.
use tokio::io::{AsyncBufReadExt, AsyncRead};

/// Harmless chatter, by kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Noise {
    /// Mesa probing a GPU it has no driver for, then using one that works.
    Gpu,
    /// Chromium's X11 atom cache meeting a drop type it hadn't cached.
    X11,
}

impl Noise {
    fn note(self) -> &'static str {
        match self {
            Noise::Gpu => {
                "GPU device-probe messages from Electron (harmless: a GPU with no Mesa driver, probed and skipped)"
            }
            Noise::X11 => {
                "Chromium's X11 drag-and-drop atom-cache messages (harmless: a drop type it had not cached, added on the spot)"
            }
        }
    }
}

/// The kind of noise `line` is, or `None`: a line to keep. Exact shapes
/// only, so a real error never matches a pattern that grew a little.
pub fn noise(line: &str) -> Option<Noise> {
    let l = line.trim();
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let word = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    let gpu = l.starts_with("KMS: DRM_IOCTL_MODE_CREATE_DUMB failed")
        || l.strip_prefix("pci id for fd ").and_then(|r| r.split_once(':')).is_some_and(|(fd, _)| digits(fd))
        || l.starts_with("MESA-LOADER: failed to open")
        || l.starts_with("MESA-LOADER: failed to retrieve")
        || l.strip_prefix("failed to load driver: ").is_some_and(word);
    if gpu {
        return Some(Noise::Gpu);
    }
    // `[pid:tid:date/time:ERROR:<dir/>atom_cache.cc(N)] Add <type> to kAtomsToCache`
    // (`:N]` in newer builds).
    let x11 = (|| {
        let (stamp, rest) = l.strip_prefix('[')?.split_once(":ERROR:")?;
        let (file, msg) = rest.split_once("] ")?;
        let at = file.rfind("atom_cache.cc")?;
        let (dir, tail) = (&file[..at], &file[at + "atom_cache.cc".len()..]);
        let line_no = tail.strip_prefix('(').and_then(|t| t.strip_suffix(')')).or(tail.strip_prefix(':'))?;
        let ty = msg.strip_prefix("Add ")?.strip_suffix(" to kAtomsToCache")?;
        let path = |b: u8| b.is_ascii_alphanumeric() || b"_./".contains(&b);
        (!stamp.is_empty()
            && stamp.bytes().all(|b| b.is_ascii_digit() || b":/.".contains(&b))
            && (dir.is_empty() || dir.ends_with('/') && dir.bytes().all(path))
            && digits(line_no)
            && !ty.is_empty()
            && !ty.contains(char::is_whitespace))
        .then_some(())
    })();
    x11.map(|()| Noise::X11)
}

/// Did the runtime end because its GPU can't be used (Chromium gives up
/// after the GPU process keeps failing)? `tail`: its last lines.
pub fn gpu_failed(tail: &[String]) -> bool {
    tail.iter().any(|l| {
        l.contains("GPU process isn't usable") || l.contains("Exiting GPU process due to errors during initialization")
    })
}

/// What a line saying the X server is full (Xlib's `Maximum number of
/// clients reached`) means, in one line; `None`: not that line.
pub fn full_x(line: &str, display: &str) -> Option<String> {
    line.contains("Maximum number of clients reached").then(|| {
        format!(
            "[aio] the X server {display} is full (\"Maximum number of clients reached\"): programs that never \
             closed their connections hold it (`xlsclients -display {display}` lists them); close them or restart the X session"
        )
    })
}

/// How many of its last lines a runtime that fails is quoted by.
const TAIL: usize = 5;

/// Carry `r` (the runtime's stderr) into this process's (the app's log),
/// noise dropped; returns the last lines kept, for the error when the
/// runtime ends badly.
pub async fn forward(r: impl AsyncRead + Unpin) -> Vec<String> {
    let mut lines = tokio::io::BufReader::new(r).lines();
    let (mut said, mut tail, mut full) = (vec![], std::collections::VecDeque::new(), false);
    while let Ok(Some(l)) = lines.next_line().await {
        match noise(&l) {
            Some(n) => {
                if !said.contains(&n) {
                    said.push(n);
                    eprintln!("[aio] suppressing {}", n.note());
                }
            }
            None => {
                eprintln!("{l}");
                if !full && let Some(why) = full_x(&l, &std::env::var("DISPLAY").unwrap_or_default()) {
                    full = true;
                    eprintln!("{why}");
                }
                if !l.trim().is_empty() {
                    tail.push_back(l);
                    if tail.len() > TAIL {
                        tail.pop_front();
                    }
                }
            }
        }
    }
    tail.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_x_server_is_said_in_one_line() {
        let l = full_x("Maximum number of clients reached", ":0").unwrap();
        assert!(l.contains(":0 is full") && l.contains("xlsclients -display :0") && !l.contains('\n'), "{l}");
        assert_eq!(full_x("[1:2:ERROR:x11_util.cc] other", ":0"), None);
    }

    #[test]
    fn gpu_probe_noise_is_dropped_in_exactly_those_shapes() {
        // aio tests/electron-renderer-log.test.ts, both lists.
        for l in [
            "KMS: DRM_IOCTL_MODE_CREATE_DUMB failed: Permission denied",
            "pci id for fd 21: 10de:2204, driver (null)",
            "MESA-LOADER: failed to open nouveau: /usr/lib/dri/nouveau_dri.so",
            "MESA-LOADER: failed to retrieve device information",
            "failed to load driver: nouveau",
            "  KMS: DRM_IOCTL_MODE_CREATE_DUMB failed: Permission denied  ",
        ] {
            assert_eq!(noise(l), Some(Noise::Gpu), "{l}");
        }
        for l in [
            "failed to load driver: nouveau (and then the app crashed)",
            "FATAL:setuid_sandbox_host.cc(166)] The SUID sandbox helper binary was found",
            "Permission denied",
            "[1234:0828/165000.123:ERROR:gpu_init.cc(523)] Passthrough is not supported",
            "pci id for fd x: 10de",
            "",
        ] {
            assert_eq!(noise(l), None, "{l}");
        }
    }

    #[test]
    fn x11_atom_cache_noise_is_dropped_in_that_shape_only() {
        for l in [
            "[48213:48213:1020/141502.318204:ERROR:atom_cache.cc(229)] Add application/vnd.portal.files to kAtomsToCache",
            "[48213:48213:1020/141502.318204:ERROR:ui/gfx/x/atom_cache.cc:229] Add application/vnd.portal.filetransfer to kAtomsToCache",
            "[9:9:1020/1.2:ERROR:atom_cache.cc(229)] Add text/x-moz-url to kAtomsToCache",
        ] {
            assert_eq!(noise(l), Some(Noise::X11), "{l}");
        }
        for l in [
            "[48213:48213:1020/141502.318204:ERROR:atom_cache.cc(229)] Add a to kAtomsToCache, then crashed",
            "[48213:48213:1020/141502.318204:ERROR:atom_cache.cc(229)] something else entirely",
            "[48213:48213:1020/141502.318204:ERROR:x11_atom_cache_evil.cc(1)] Add x to kAtomsToCache",
            "Add application/vnd.portal.files to kAtomsToCache",
            "[1:ERROR:atom_cache.cc()] Add x to kAtomsToCache",
        ] {
            assert_eq!(noise(l), None, "{l}");
        }
    }

    #[test]
    fn a_gpu_that_cant_be_used_is_told_from_other_ends() {
        let t = |l: &str| vec!["x".to_string(), l.to_string()];
        assert!(gpu_failed(&t(
            "[9:9:0101/1.2:FATAL:gpu_data_manager_impl_private.cc(415)] GPU process isn't usable. Goodbye."
        )));
        assert!(gpu_failed(&t(
            "[1:1:ERROR:viz_main_impl.cc(183)] Exiting GPU process due to errors during initialization"
        )));
        assert!(!gpu_failed(&t("[1:1:FATAL:render_process.cc] renderer gone")));
        assert!(!gpu_failed(&[]));
    }

    #[tokio::test]
    async fn the_rest_is_kept_and_the_last_lines_are_returned() {
        let err = "a\nKMS: DRM_IOCTL_MODE_CREATE_DUMB failed: x\n\nb\nc\nd\ne\nf\nFATAL: gone\n";
        assert_eq!(forward(err.as_bytes()).await, ["c", "d", "e", "f", "FATAL: gone"]);
        assert!(forward(&b"pci id for fd 3: x\n"[..]).await.is_empty());
    }
}

//! Whether the X server has room for the window (Linux). An X server serves
//! at most so many clients (Xorg: 256 unless `MaxClients` says more); one
//! with fewer than a window's connections left still lets the runtime's
//! main process in, but not its GPU process: the window opens and stays
//! empty, its page drawn where no one sees it. Programs that never close
//! their connections fill it (a private D-Bus's portals and at-spi left
//! behind), so the app says that instead of opening empty. The probe is a
//! few local connection setups (about a millisecond), closed at once but
//! one, kept while the app runs (see [`full`]).
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The connections a window takes: measured on Electron 44 (Xvfb, a page and
/// its GPU process), 3 left opens it empty, 4 draws it.
pub const NEED: usize = 4;

/// The local socket and number of X `display` (`:N`, `:N.S`, `unix:N`);
/// `None`: not a local one (a TCP display, a malformed name).
pub fn socket(display: &str) -> Option<(PathBuf, String)> {
    let n = display.strip_prefix("unix").unwrap_or(display).strip_prefix(':')?;
    let n = n.split_once('.').map_or(n, |(n, _)| n);
    n.parse::<u32>().ok().map(|n| (PathBuf::from(format!("/tmp/.X11-unix/X{n}")), n.to_string()))
}

/// The `MIT-MAGIC-COOKIE-1` for display number `n` on `host` in an
/// Xauthority file's bytes, as Xlib picks it (this host or any, this
/// display or any).
pub fn cookie(auth: &[u8], host: &[u8], n: &str) -> Option<Vec<u8>> {
    let mut at = auth;
    let mut take = |len: usize| -> Option<&[u8]> {
        let (a, b) = (at.get(..len)?, at.get(len..)?);
        at = b;
        Some(a)
    };
    loop {
        let family = u16::from_be_bytes(take(2)?.try_into().ok()?);
        let mut field = || -> Option<Vec<u8>> {
            let len = u16::from_be_bytes(take(2)?.try_into().ok()?) as usize;
            Some(take(len)?.to_vec())
        };
        let (addr, num, name, data) = (field()?, field()?, field()?, field()?);
        let here = family == 0xffff || (family == 256 && addr == host);
        if here && (num.is_empty() || num == n.as_bytes()) && name == b"MIT-MAGIC-COOKIE-1" {
            return Some(data);
        }
    }
}

/// The connection setup: little-endian, protocol 11.0, `cookie` if any.
fn setup(cookie: Option<&[u8]>) -> Vec<u8> {
    let (name, data): (&[u8], &[u8]) = match cookie {
        Some(c) => (b"MIT-MAGIC-COOKIE-1", c),
        None => (b"", b""),
    };
    let pad = |v: &mut Vec<u8>| v.resize(v.len().next_multiple_of(4), 0);
    let mut s = vec![0x6c, 0, 11, 0, 0, 0];
    s.extend((name.len() as u16).to_le_bytes());
    s.extend((data.len() as u16).to_le_bytes());
    s.extend([0, 0]);
    s.extend(name);
    pad(&mut s);
    s.extend(data);
    pad(&mut s);
    s
}

/// Whether the X server at `sock` turns down one of `need` connections held
/// at once as full. Anything else (no server, no answer, not let in) is not
/// this probe's to say: the runtime says it itself.
pub fn full(sock: &Path, need: usize, cookie: Option<&[u8]>) -> bool {
    let setup = setup(cookie);
    let mut held = Vec::with_capacity(need);
    for _ in 0..need {
        let Ok(mut s) = UnixStream::connect(sock) else { return false };
        let mut head = [0u8; 8];
        if s.set_read_timeout(Some(Duration::from_secs(2))).is_err()
            || s.write_all(&setup).is_err()
            || s.read_exact(&mut head).is_err()
        {
            return false;
        }
        if head[0] != 1 {
            let mut reason = vec![0u8; head[1] as usize];
            return head[0] == 0
                && s.read_exact(&mut reason).is_ok()
                && String::from_utf8_lossy(&reason).contains("Maximum number of clients reached");
        }
        held.push(s);
    }
    // One stays open while this process runs: a server started with
    // `-terminate` (`ax start`'s, a smoke's Xvfb) ends when its last client
    // goes, and that would be this probe, before the runtime connects.
    if let Some(s) = held.pop() {
        _ = KEPT.set(s);
    }
    false
}

/// The probe's connection kept open (see [`full`]).
static KEPT: std::sync::OnceLock<UnixStream> = std::sync::OnceLock::new();

/// Why the window can't open on this X display, if it can't (`env`: the
/// process's variables). A Wayland session is none of this.
pub fn refuses(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if env("WAYLAND_DISPLAY").is_some_and(|w| !w.is_empty()) {
        return None;
    }
    let d = env("DISPLAY")?;
    let (sock, n) = socket(&d)?;
    let file = env("XAUTHORITY").map(PathBuf::from).or_else(|| env("HOME").map(|h| Path::new(&h).join(".Xauthority")));
    let host = std::fs::read("/proc/sys/kernel/hostname").unwrap_or_default();
    let c = file.and_then(|f| std::fs::read(f).ok()).and_then(|a| cookie(&a, host.trim_ascii_end(), &n));
    full(&sock, NEED, c.as_deref()).then(|| {
        format!(
            "the X server {d} is full (\"Maximum number of clients reached\"): the window would open empty. \
             Programs that never closed their connections hold it (`xlsclients -display {d}` lists them; \
             leftover xdg-desktop-portal and at-spi2-registryd processes are the usual ones): close them, \
             or restart the X session"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn local_displays_have_a_socket_and_others_none() {
        assert_eq!(socket(":0"), Some((PathBuf::from("/tmp/.X11-unix/X0"), "0".into())));
        assert_eq!(socket(":94.0"), Some((PathBuf::from("/tmp/.X11-unix/X94"), "94".into())));
        assert_eq!(socket("unix:3").map(|s| s.1), Some("3".into()));
        assert_eq!(socket("host:0"), None);
        assert_eq!(socket(""), None);
    }

    fn entry(family: u16, addr: &[u8], num: &[u8], name: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = family.to_be_bytes().to_vec();
        for f in [addr, num, name, data] {
            v.extend((f.len() as u16).to_be_bytes());
            v.extend(f);
        }
        v
    }

    #[test]
    fn the_cookie_is_this_hosts_for_this_display() {
        let mut auth = entry(256, b"other", b"0", b"MIT-MAGIC-COOKIE-1", b"theirs");
        auth.extend(entry(256, b"box", b"1", b"MIT-MAGIC-COOKIE-1", b"display one"));
        auth.extend(entry(256, b"box", b"0", b"XDM-AUTHORIZATION-1", b"not this kind"));
        auth.extend(entry(256, b"box", b"0", b"MIT-MAGIC-COOKIE-1", b"mine"));
        assert_eq!(cookie(&auth, b"box", "0").as_deref(), Some(&b"mine"[..]));
        assert_eq!(cookie(&auth, b"box", "7"), None);
        let wild = entry(0xffff, b"", b"7", b"MIT-MAGIC-COOKIE-1", b"any host");
        assert_eq!(cookie(&wild, b"box", "7").as_deref(), Some(&b"any host"[..]));
        assert_eq!(cookie(&auth[..auth.len() - 3], b"box", "0"), None, "a cut file is none");
        // The setup carries it, every part padded to 4.
        let s = setup(Some(b"mine"));
        assert_eq!((s.len(), &s[6..8], &s[8..10]), (12 + 20 + 4, &[18u8, 0][..], &[4u8, 0][..]));
    }

    /// A server that lets `room` clients in and refuses the next as full;
    /// `auth`: it lets none in, saying authorisation is required.
    fn server(room: usize, auth: bool) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let sock = d.path().join("X9");
        let l = UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || {
            let mut kept = vec![];
            for (i, s) in l.incoming().enumerate() {
                let mut s = s.unwrap();
                let mut setup = [0u8; 12];
                s.read_exact(&mut setup).unwrap();
                let reply = match (i < room, auth) {
                    (true, false) => vec![1, 0, 11, 0, 0, 0, 0, 0],
                    (true, true) => [&[0u8, 22, 11, 0, 0, 0, 0, 0][..], b"Authorization required"].concat(),
                    (false, _) => [&[0u8, 33, 11, 0, 0, 0, 0, 0][..], b"Maximum number of clients reached"].concat(),
                };
                s.write_all(&reply).unwrap();
                kept.push(s);
            }
        });
        (d, sock)
    }

    #[test]
    fn a_server_with_fewer_free_clients_than_a_window_needs_is_full() {
        let (_d, sock) = server(NEED - 1, false);
        assert!(full(&sock, NEED, None));
        let (_d, sock) = server(NEED, false);
        assert!(!full(&sock, NEED, None), "room for every connection a window takes");
        let (_d, sock) = server(NEED, true);
        assert!(!full(&sock, NEED, None), "not let in: not this probe's to say");
        assert!(!full(Path::new("/nonexistent/X9"), NEED, None), "no server either");
    }

    #[test]
    fn a_wayland_session_is_not_asked() {
        let env = |k: &str| match k {
            "WAYLAND_DISPLAY" => Some("wayland-0".to_string()),
            "DISPLAY" => Some(":0".to_string()),
            _ => None,
        };
        assert_eq!(refuses(&env), None);
    }
}

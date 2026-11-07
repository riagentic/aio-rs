//! A window's client and its connect page (aio's thin client): where the
//! server is, when the app can't tell by itself. A client opened with
//! [`crate::Source::for_app`] in a binary with the kit (a window) never
//! fails for want of a server: with
//! none named, `--connect`, one that can't be reached (and no copy saved)
//! or one that refuses, its copy waits and the window shows the connect
//! page (`aio_shell` draws it from [`page`]) until the person picks one: an
//! address, a server on the LAN, a pairing code, a recent server. The page
//! goes the moment a server answers.
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::LazyLock;
#[cfg(feature = "server")]
use std::sync::Mutex;

/// A server this client reached (`servers.json` in its data, 0600: a
/// paired one's key is in it; newest first).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    /// `host:port`.
    pub addr: String,
    /// Its app's name, when it said.
    #[serde(default)]
    pub title: Option<String>,
    /// Paired: the app's key, and the root pinned (TLS).
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub root: Option<String>,
}

/// A server on the LAN that answered (`[server] discover`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Near {
    /// `host:port`.
    pub addr: String,
    pub title: Option<String>,
    /// It needs its key: pair with the code it shows.
    pub auth: bool,
}

/// How many recent servers are kept.
const KEEP: usize = 10;

fn file(data: &Path) -> std::path::PathBuf {
    data.join("servers.json")
}

/// The servers this client reached, newest first.
pub fn recent(data: &Path) -> Vec<Server> {
    std::fs::read(file(data)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// `s` first among the recent servers (once, the oldest past ten gone).
pub fn remember(data: &Path, s: Server) -> std::io::Result<()> {
    let mut all = recent(data);
    all.retain(|o| o.addr != s.addr);
    all.insert(0, s);
    all.truncate(KEEP);
    crate::paths::write_atomic(&file(data), serde_json::to_string(&all)?.as_bytes(), true)
}

/// `addr` off the recent servers (its pairing, if it is this client's
/// server, stays).
pub fn forget(data: &Path, addr: &str) -> std::io::Result<()> {
    let mut all = recent(data);
    all.retain(|o| o.addr != addr);
    crate::paths::write_atomic(&file(data), serde_json::to_string(&all)?.as_bytes(), true)
}

/// Why the connect page shows; `None`: the app does.
static PAGE: LazyLock<tokio::sync::watch::Sender<Option<String>>> =
    LazyLock::new(|| tokio::sync::watch::Sender::new(None));

/// The connect page's reason, as it changes (`None`: show the app).
pub fn page() -> tokio::sync::watch::Receiver<Option<String>> {
    PAGE.subscribe()
}

/// Show the connect page, saying `why`.
pub fn ask(why: impl Into<String>) {
    PAGE.send_replace(Some(why.into()));
}

/// A server answered: the app shows.
#[cfg(feature = "server")]
pub(crate) fn reached() {
    PAGE.send_replace(None);
}

/// Where this process's window client goes next (`Remote::relink`).
#[cfg(feature = "server")]
type Relink = std::sync::Arc<dyn Fn(crate::sync::Link) + Send + Sync>;
#[cfg(feature = "server")]
static RELINK: Mutex<Option<Relink>> = Mutex::new(None);

/// This process has a window's client: its window gates the app on the
/// connect page ([`page`]).
#[cfg(feature = "server")]
pub fn active() -> bool {
    RELINK.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// This process has a window's client: never without `server` (no client
/// mode), so a local app's binary carries no connect page.
#[cfg(not(feature = "server"))]
pub fn active() -> bool {
    false
}

#[cfg(feature = "server")]
pub(crate) fn wire(relink: impl Fn(crate::sync::Link) + Send + Sync + 'static) {
    *RELINK.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(relink));
}

/// The link this client keeps (`paired.json`, 0600: it holds the key).
#[cfg(feature = "server")]
pub(crate) fn saved(data: &Path) -> Option<crate::sync::Link> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(data.join("paired.json")).ok()?).ok()?;
    Some(crate::sync::Link {
        addr: v["addr"].as_str()?.into(),
        token: v["token"].as_str()?.into(),
        root: v["root"].as_str().map(String::from),
    })
}

/// Keep `link` as this client's, and point its copy there: the person's
/// pick, so reached whatever `[capabilities] network` says.
#[cfg(feature = "server")]
pub(crate) fn keep(data: &Path, link: crate::sync::Link) -> std::io::Result<()> {
    crate::capabilities::grant(&link.addr);
    crate::paths::private_dir(data)?;
    let v = serde_json::json!({ "addr": link.addr, "token": link.token, "root": link.root });
    // A new pairing replaces the old one.
    crate::paths::write_atomic(&data.join("paired.json"), v.to_string().as_bytes(), true)?;
    let go = RELINK.lock().unwrap_or_else(|e| e.into_inner()).clone();
    go.into_iter().for_each(|f| f(link.clone()));
    Ok(())
}

/// Connect to the server at `addr` (`host:port`) with no key, no pairing: a
/// server open on purpose (`[server] open`) on a network you trust, over
/// plain sockets. This app's own key never goes to a server nothing
/// verified; one with a key is paired with ([`pair`]).
pub fn open_at(data: &Path, addr: &str) -> std::io::Result<()> {
    #[cfg(feature = "server")]
    {
        let addr = addr.trim();
        crate::capabilities::host_port(addr).1.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{addr:?}: an address is host:port"))
        })?;
        crate::paths::private_dir(data)?;
        let link = crate::sync::Link { addr: addr.into(), token: String::new(), root: None };
        keep(data, link)?;
        remember(data, Server { addr: addr.into(), ..Server::default() })
    }
    #[cfg(not(feature = "server"))]
    {
        _ = (data, addr);
        Err(crate::left_out("server", "a client's server"))
    }
}

/// Connect to recent server `s` again: with its kept key and pin when it
/// was paired, else as [`open_at`].
pub fn again(data: &Path, s: &Server) -> std::io::Result<()> {
    #[cfg(feature = "server")]
    if let Some(token) = &s.token {
        keep(data, crate::sync::Link { addr: s.addr.clone(), token: token.clone(), root: s.root.clone() })?;
        return remember(data, s.clone());
    }
    open_at(data, &s.addr)
}

/// Pair with app `app`'s server at `addr` by the code it shows, keep the
/// link (its key, its root pinned) and point this client's copy there.
/// Whether it is TLS.
pub async fn pair(data: &Path, app: &str, addr: &str, code: &str) -> std::io::Result<bool> {
    #[cfg(feature = "tls")]
    {
        let addr = addr.trim();
        // The pick lets the pairing through; it stays granted only once the
        // pairing worked with this app's server (`keep` grants the link).
        let had = crate::capabilities::check("pairing", addr).is_ok();
        crate::capabilities::grant(addr);
        let p = crate::pairing::pair(addr, code.trim()).await.and_then(|p| match p.name == app {
            true => Ok(p),
            false => Err(std::io::Error::other(format!("{addr} serves {}, not {app}", p.name))),
        });
        if !had && p.is_err() {
            crate::capabilities::revoke(addr);
        }
        let p = p?;
        let host = addr.rsplit_once(':').map_or(addr, |(h, _)| h);
        let link = crate::sync::Link { addr: format!("{host}:{}", p.port), token: p.key.clone(), root: p.cert.clone() };
        let s = Server { addr: link.addr.clone(), title: Some(p.name.clone()), token: Some(p.key), root: p.cert };
        keep(data, link)?;
        remember(data, s)?;
        Ok(p.tls)
    }
    #[cfg(not(feature = "tls"))]
    {
        _ = (data, app, addr, code);
        Err(crate::left_out("tls", "pairing"))
    }
}

/// App `app`'s servers on the LAN that answer within `wait` (on every
/// network this machine is on, [`crate::discovery::probe_lan`]).
pub async fn lan(app: &str, wait: std::time::Duration) -> std::io::Result<Vec<Near>> {
    #[cfg(feature = "tls")]
    {
        let found = crate::discovery::probe_lan(crate::discovery::port(), wait).await?;
        let near = found.into_iter().filter(|f| f.name == app);
        Ok(near
            .filter_map(|f| Some(Near { addr: std::net::SocketAddr::new(f.host?, f.port).to_string(), title: f.title, auth: f.needs_auth }))
            .collect())
    }
    #[cfg(not(feature = "tls"))]
    {
        _ = (app, wait);
        Ok(vec![])
    }
}

/// The link a launch's `--server-url` names (pure): `host:port`, or an
/// `http(s)://` / `ws(s)://` URL whose `?token=` is the key. The key, else
/// the pairing's when it is that server, else `own` (this client's); a
/// TLS server only with the root its pairing pinned.
#[cfg(feature = "server")]
pub(crate) fn url_link(url: &str, paired: Option<&crate::sync::Link>, own: &str) -> Result<crate::sync::Link, String> {
    let (tls, rest) = match url.split_once("://") {
        Some(("https" | "wss", r)) => (true, r),
        Some(("http" | "ws", r)) => (false, r),
        Some((s, _)) => return Err(format!("--server-url {url}: http(s) or ws(s), not {s}")),
        None => (false, url),
    };
    let (addr, query) = rest.split_once(['/', '?']).map_or((rest, ""), |(a, _)| (a, rest.get(a.len()..).unwrap_or("")));
    if crate::capabilities::host_port(addr).1.is_none() {
        return Err(format!("--server-url {url}: name host:port"));
    }
    let token = query.split(['?', '&']).find_map(|kv| kv.strip_prefix("token=")).map(|t| {
        percent_encoding::percent_decode_str(t).decode_utf8_lossy().into_owned()
    });
    let same = paired.filter(|l| l.addr == addr);
    let root = same.and_then(|l| l.root.clone());
    if tls && root.is_none() {
        return Err(format!(
            "--server-url {url}: a TLS server is reached pinned to its root: pair with it first (its connect page, \
             or ax pair)"
        ));
    }
    let token = token.or_else(|| same.map(|l| l.token.clone())).unwrap_or_else(|| own.into());
    Ok(crate::sync::Link { addr: addr.into(), token, root })
}

/// What a launch says when its `--server-url` holds the key (`?token=`)
/// (pure): a command line is seen by every user of the machine (`ps`).
#[cfg(feature = "server")]
pub(crate) fn key_in_argv(url: &str) -> Option<&'static str> {
    url.split_once('?').is_some_and(|(_, q)| q.split('&').any(|kv| kv.starts_with("token="))).then_some(
        "[aio] security: --server-url holds the app key (?token=), which every user of this machine sees in its \
         command line. Give it in AIORT_TOKEN or data/sync.token instead",
    )
}

/// `link`, its pin checked: a server that shows a certificate the pinned
/// root doesn't cover proves it holds the app's key before anything is
/// sent ([`crate::pairing::reprove`]) and is pinned again (kept in
/// `data`); `strict` (aio's `strictPin`) refuses it instead, until it is
/// paired again. A refusal is `PermissionDenied`, saying what to do.
#[cfg(feature = "tls")]
pub(crate) async fn pinned(link: crate::sync::Link, strict: bool, data: &Path) -> std::io::Result<crate::sync::Link> {
    if crate::pairing::pin_holds(&link).await? {
        return Ok(link);
    }
    let no = |why: String| std::io::Error::new(std::io::ErrorKind::PermissionDenied, why);
    let again = "pair with it again (its code on the connect page, or ax pair)";
    if strict {
        return Err(no(format!("{}'s certificate changed and the pin is strict: nothing was sent; {again}", link.addr)));
    }
    match crate::pairing::reprove(&link.addr, &link.token).await {
        Ok(root) => {
            eprintln!("[aio] {}'s certificate changed and it proved it holds the app's key: pinned again", link.addr);
            let link = crate::sync::Link { root: Some(root), ..link };
            keep(data, link.clone())?;
            Ok(link)
        }
        Err(e) => Err(no(format!("{}'s certificate changed and {e}: its key was not sent; {again}", link.addr))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_servers_are_kept_newest_first_ten_at_most() {
        let d = tempfile::tempdir().unwrap();
        let s = |a: &str| Server { addr: a.into(), ..Server::default() };
        assert_eq!(recent(d.path()), vec![]);
        for i in 0..12 {
            remember(d.path(), s(&format!("192.0.2.{i}:8100"))).unwrap();
        }
        remember(d.path(), s("192.0.2.5:8100")).unwrap();
        let all = recent(d.path());
        assert_eq!((all.len(), all[0].addr.as_str(), all[1].addr.as_str()), (10, "192.0.2.5:8100", "192.0.2.11:8100"));
        forget(d.path(), "192.0.2.5:8100").unwrap();
        assert!(recent(d.path()).iter().all(|o| o.addr != "192.0.2.5:8100"));
    }

    #[cfg(feature = "server")]
    #[test]
    fn a_server_url_names_the_server_its_key_and_its_pin() {
        use crate::sync::Link;
        let paired = Link { addr: "192.0.2.7:8100".into(), token: "paired".into(), root: Some("ROOT".into()) };
        let l = url_link("192.0.2.9:8100", Some(&paired), "own").unwrap();
        assert_eq!((l.addr.as_str(), l.token.as_str(), l.root), ("192.0.2.9:8100", "own", None));
        let l = url_link("http://192.0.2.9:8100/?token=a%2Bb", None, "own").unwrap();
        assert_eq!((l.addr.as_str(), l.token.as_str()), ("192.0.2.9:8100", "a+b"));
        let l = url_link("https://192.0.2.7:8100", Some(&paired), "own").unwrap();
        assert_eq!((l.token.as_str(), l.root.as_deref()), ("paired", Some("ROOT")), "its pairing's key and pin");
        assert!(url_link("https://192.0.2.9:8100", Some(&paired), "own").unwrap_err().contains("pair with it first"));
        assert!(url_link("ftp://192.0.2.9:21", None, "own").unwrap_err().contains("not ftp"));
        assert!(url_link("192.0.2.9", None, "own").unwrap_err().contains("host:port"));
        assert!(key_in_argv("http://192.0.2.9:8100/?a=1&token=k").unwrap().contains("AIORT_TOKEN"));
        assert_eq!(key_in_argv("http://192.0.2.9:8100/?tokens=1"), None);
    }
}

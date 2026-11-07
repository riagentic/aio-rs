// Its own test binary: a window's client and its connect page are process-wide.
use aio::prelude::*;
use std::time::Duration;

#[cell(persist, shared)]
pub struct Board {
    pub notes: Vec<String>,
}

#[methods]
impl Board {
    #[client]
    pub fn add(&mut self, text: String) {
        self.notes.push(text);
    }
}

fn page() -> Option<String> {
    aio::connect::page().borrow().clone()
}

async fn until(what: &str, f: impl Fn() -> bool) {
    let end = std::time::Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(std::time::Instant::now() < end, "never: {what} (page: {:?})", page());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// aio's thin client: a window's client with no server shows its connect
/// page instead of failing; an address, then a pairing code, take it to a
/// server, the page goes, and the servers are remembered. Another server
/// picked while connected drops the live link and the copy follows it.
#[test]
fn a_window_client_with_no_server_waits_on_its_connect_page() {
    let d = tempfile::tempdir().unwrap();
    // Its own id, not its cell's (`board`): a server names its app.
    let id = "pinboard";
    let cfg = aio::config::Config::parse(&format!("[app]\nid = \"{id}\"\nmode = \"client\"")).unwrap();
    let dirs = aio::paths::AppDirs::at(d.path(), id).unwrap();
    // The launcher as 0.2's template wrote it: a binary with the kit is a window.
    assert!(!aio::connect::active());
    let s = aio::Source::<Board>::for_app(&cfg, &dirs).unwrap();
    assert!(aio::connect::active());
    assert_eq!(page().as_deref(), Some("This app has no server yet."));
    assert_eq!(s.state().notes, Vec::<String>::new(), "the cell's default until a server answers");

    // A plain server with a key: no code, no way in.
    let h = Handle::<Board>::memory();
    h.add("from the server".into()).unwrap();
    let serve = |open: bool| {
        let h = h.clone();
        aio::rt().block_on(async move {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = l.local_addr().unwrap().to_string();
            let opts = aio::sync::Options { token: "secret".into(), open, ..Default::default() };
            tokio::spawn(aio::sync::serve_with(h, l, opts));
            addr
        })
    };
    let keyed = serve(false);
    aio::connect::open_at(&dirs.data, &keyed).unwrap();
    aio::rt().block_on(until("refused", || page().is_some_and(|w| w.contains("refused"))));
    // Even holding its key by hand: a server with no code is told no key,
    // so this app's own never reaches an unverified one.
    std::fs::write(dirs.data.join("sync.token"), "secret").unwrap();
    aio::connect::open_at(&dirs.data, &keyed).unwrap();
    assert_eq!(aio::pairing::saved(&dirs.data).unwrap().token, "", "no key sent");
    // One open on purpose takes it.
    let addr = serve(true);
    aio::connect::open_at(&dirs.data, &addr).unwrap();
    aio::rt().block_on(until("synced", || page().is_none()));
    assert_eq!(s.state().notes, ["from the server"]);
    assert_eq!(aio::connect::recent(&dirs.data)[0].addr, addr);
    // The next start finds it: no page.
    assert_eq!(aio::pairing::saved(&dirs.data).unwrap().addr, addr);

    // Pairing by code: TLS, its root pinned, its key kept for the recent list.
    let roots = tempfile::tempdir().unwrap();
    let tls = aio::tls::server(roots.path(), id).unwrap();
    let pairing = std::sync::Arc::new(aio::pairing::Pairing::default());
    let other = Handle::<Board>::memory();
    other.add("from the paired server".into()).unwrap();
    let paired = aio::rt().block_on(async {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let opts = aio::sync::Options {
            token: "other".into(),
            pairing: Some(pairing.clone()),
            app: id.into(),
            tls: Some(tls),
            ..Default::default()
        };
        tokio::spawn(aio::sync::serve_with(other.clone(), l, opts));
        addr
    });
    let code = pairing.new_pake_pin();
    let e = aio::rt().block_on(aio::connect::pair(&dirs.data, "notes", &paired, &code)).unwrap_err();
    assert!(e.to_string().contains("serves pinboard, not notes"), "{e}");
    let code = pairing.new_pake_pin();
    assert!(aio::rt().block_on(aio::connect::pair(&dirs.data, id, &paired, &code)).unwrap());
    let recent = aio::connect::recent(&dirs.data);
    assert_eq!(
        (recent[0].addr.as_str(), recent[0].token.as_deref(), recent.len()),
        (paired.as_str(), Some("other"), 3)
    );
    assert!(recent[0].root.is_some());
    // Paired while connected: the live link drops, the copy is the new server's.
    aio::rt().block_on(until("moved to the paired server", || s.state().notes == ["from the paired server"]));
    // Back to the first from the recent list, at once (no wait for a drop).
    aio::connect::again(&dirs.data, &recent[1]).unwrap();
    aio::rt().block_on(until("moved back", || s.state().notes == ["from the server"]));
    assert!(page().is_none());
    // A server on another machine, picked here: reached with no network entry.
    aio::capabilities::set(&[]);
    assert!(aio::capabilities::check("sync", "192.0.2.9:8100").is_err());
    aio::connect::open_at(&dirs.data, "192.0.2.9:8100").unwrap();
    assert!(aio::capabilities::check("sync", "192.0.2.9:8100").is_ok());
    // A pairing that came to nothing leaves no way out to its address.
    assert!(aio::rt().block_on(aio::connect::pair(&dirs.data, id, "0.0.0.0:9", "12345678")).is_err());
    assert!(aio::capabilities::check("sync", "0.0.0.0:9").is_err(), "not granted");
}

//! The app's local door: where this user's tools (`ax state`, `ax call`,
//! `ax metrics`, time travel) and other apps ([`crate::peer`]) reach a
//! running cell. A private socket (`ipc`: the OS admits only this user),
//! so no key and no network, in every build, `net` or not. Any process of
//! this user may open it, so only a debug build's door has the operator's
//! rights (any method, `#[server]` fields, time travel, a loaded state); a
//! release build's gets what a client with the key gets (`#[client]`
//! methods as their rules say, what `visible` shows), as a peer does.
//!
//! Wire: the client sends [`VERSION`] (one byte), then both sides send
//! frames: a big-endian `u32` length, then a kind byte and its payload: a
//! JSON message (`TEXT`, the protocol of `sync`'s `/ws`), a ping, its pong,
//! or a close with its code and why. A version the app doesn't speak is
//! told so in a close and dropped.
use crate::link::{Calls, Gate, Msg, Sock};
use crate::{Cell, Handle};
use bytes::{BufMut, Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

pub use crate::link::Limits;

/// The door's wire version, the first byte a client sends.
pub const VERSION: u8 = 1;

const TEXT: u8 = 0;
const PING: u8 = 1;
const PONG: u8 = 2;
const CLOSE: u8 = 3;

/// A door link's frames.
pub(crate) type Frames = Framed<crate::ipc::Stream, LengthDelimitedCodec>;

/// `s` as frames (its version byte already sent or read).
pub(crate) fn frames(s: crate::ipc::Stream) -> Frames {
    Framed::new(s, LengthDelimitedCodec::builder().max_frame_length(crate::link::MAX_MESSAGE + 1).new_codec())
}

/// Is `addr` a local door (`ipc::addr`: a socket path or a Windows pipe)
/// rather than `host:port`?
pub fn is_door(addr: &str) -> bool {
    addr.starts_with('/') || addr.starts_with(r"\\.\pipe\")
}

/// Where app `app` (data dir `data`) serves cell `cell` to this user's tools.
pub fn at(app: &str, data: &std::path::Path, cell: &str) -> std::io::Result<String> {
    Ok(crate::ipc::addr(&crate::paths::sockets(app, data)?, &format!("cell-{cell}")))
}

/// The cells whose doors the running app `app` (data dir `data`) has open
/// (its socket folder; on Windows, the pipes named for it), sorted.
pub fn open(app: &str, data: &std::path::Path) -> Vec<String> {
    let dir = crate::paths::sockets_at(app, data);
    let (at, prefix) = match cfg!(windows) {
        true => (
            std::path::PathBuf::from(r"\\.\pipe\"),
            crate::ipc::addr(&dir, "cell-").rsplit('\\').next().unwrap_or_default().to_string(),
        ),
        false => (dir, "cell-".to_string()),
    };
    let names = std::fs::read_dir(at).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned());
    let mut cells: Vec<String> =
        names.filter_map(|n| Some(n.strip_prefix(&prefix)?.trim_end_matches(".sock").to_string())).collect();
    cells.sort();
    cells
}

/// One frame's bytes (pure).
pub(crate) fn encode(m: Msg) -> Bytes {
    let (kind, body) = match m {
        Msg::Text(t) => (TEXT, t.into_bytes()),
        Msg::Ping(p) => (PING, p),
        Msg::Pong(p) => (PONG, p),
        Msg::Close(code, why) => (CLOSE, [&code.to_be_bytes()[..], why.as_bytes()].concat()),
    };
    let mut b = BytesMut::with_capacity(1 + body.len());
    b.put_u8(kind);
    b.put_slice(&body);
    b.freeze()
}

/// One frame read back; `None` for one no version of this wire sends (pure).
pub(crate) fn decode(b: &[u8]) -> Option<Msg> {
    let (&kind, body) = b.split_first()?;
    Some(match kind {
        TEXT => Msg::Text(String::from_utf8(body.to_vec()).ok()?),
        PING => Msg::Ping(body.to_vec()),
        PONG => Msg::Pong(body.to_vec()),
        CLOSE if body.len() >= 2 => {
            Msg::Close(u16::from_be_bytes([body[0], body[1]]), String::from_utf8_lossy(&body[2..]).into_owned())
        }
        _ => return None,
    })
}

/// The next frame; `None` once the link is gone or sent what isn't a frame.
pub(crate) async fn recv(f: &mut Frames) -> Option<Msg> {
    decode(&f.next().await?.ok()?)
}

pub(crate) async fn send(f: &mut Frames, m: Msg) -> std::io::Result<()> {
    f.send(encode(m)).await
}

/// A client's end of door `addr`: connected, its version said.
#[cfg(any(feature = "server", feature = "compat"))]
pub(crate) async fn dial(addr: &str) -> std::io::Result<Frames> {
    let mut s = crate::ipc::connect(addr).await?;
    tokio::io::AsyncWriteExt::write_u8(&mut s, VERSION).await?;
    Ok(frames(s))
}

/// Whether this build's door has the operator's rights: `ax dev`'s build
/// only (aio's `dev` feature).
pub const OPERATOR: bool = cfg!(feature = "dev");

/// Serve `h` to this user's tools on the local door `l` (at [`at`]), until
/// it fails: `sync`'s protocol without a key; in `ax dev`'s build ([`OPERATOR`])
/// tools there may also call any method, list the open links, travel in
/// time and load a state.
pub async fn serve<T: Cell>(h: Handle<T>, l: crate::ipc::Listener, limits: Limits) -> std::io::Result<()> {
    serve_with(h, l, limits, crate::link::no_calls(), OPERATOR, Default::default()).await
}

/// [`serve`], each link running `calls`' server functions, heard and its
/// peers resolved by `hooks`.
pub(crate) async fn serve_with<T: Cell>(
    h: Handle<T>,
    mut l: crate::ipc::Listener,
    limits: Limits,
    calls: Calls,
    operator: bool,
    hooks: crate::link::Hooks,
) -> std::io::Result<()> {
    let gate = if operator { Gate::Door } else { Gate::Local };
    let open = Arc::new(AtomicUsize::new(0));
    loop {
        let Ok(s) = l.accept().await else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        let (h, calls, open, gate, hooks) = (h.clone(), calls.clone(), open.clone(), gate.clone(), hooks.clone());
        tokio::spawn(async move {
            if let Err(e) = link(h, s, limits, calls, gate, hooks, &open).await {
                eprintln!("[aio] door: {e}");
            }
        });
    }
}

/// One link on the door, until it leaves.
async fn link<T: Cell>(
    h: Handle<T>,
    mut s: crate::ipc::Stream,
    limits: Limits,
    calls: Calls,
    gate: Gate,
    hooks: crate::link::Hooks,
    open: &AtomicUsize,
) -> Result<(), String> {
    let v = tokio::time::timeout(Duration::from_secs(15), s.read_u8())
        .await
        .map_err(|_| "a client said nothing".to_string())?
        .map_err(|e| e.to_string())?;
    let mut f = frames(s);
    if v != VERSION {
        let why = format!("this app's door speaks v{VERSION}, the client v{v}: run the ax that built it");
        _ = send(&mut f, Msg::Close(1002, why.clone())).await;
        return Err(format!("refused a client: {why}"));
    }
    if open.fetch_add(1, Ordering::SeqCst) >= limits.clients {
        open.fetch_sub(1, Ordering::SeqCst);
        let why = format!("too many clients (the limit is {}): try again later", limits.clients);
        _ = send(&mut f, Msg::Close(1013, why.clone())).await;
        return Err(why);
    }
    let r = crate::link::session(h, Sock::Door(f), limits, "door".into(), gate, None, calls, hooks, None).await;
    open.fetch_sub(1, Ordering::SeqCst);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_read_back_as_sent() {
        let back = |m: Msg| match decode(&encode(m)) {
            Some(Msg::Text(t)) => format!("text {t}"),
            Some(Msg::Ping(p)) => format!("ping {p:?}"),
            Some(Msg::Pong(p)) => format!("pong {p:?}"),
            Some(Msg::Close(c, w)) => format!("close {c} {w}"),
            None => "none".into(),
        };
        assert_eq!(back(Msg::Text(r#"{"t":"hello"}"#.into())), r#"text {"t":"hello"}"#);
        assert_eq!(back(Msg::Ping(vec![1, 2])), "ping [1, 2]");
        assert_eq!(back(Msg::Pong(vec![])), "pong []");
        assert_eq!(back(Msg::Close(1009, "too big".into())), "close 1009 too big");
        assert!(decode(&[]).is_none() && decode(&[9, 1]).is_none() && decode(&[CLOSE, 3]).is_none());
        assert!(decode(&[TEXT, 0xff]).is_none(), "text is UTF-8");
    }

    #[crate::cell(shared)]
    pub struct Vault {
        pub n: u32,
        #[server]
        pub secret: crate::Hidden<String>,
    }

    #[crate::methods]
    impl Vault {
        #[client]
        pub fn bump(&mut self) -> crate::Result<u32> {
            self.n += 1;
            Ok(self.n)
        }
        pub fn spend(&mut self) -> crate::Result {
            self.n = 0;
            Ok(())
        }
    }

    /// A tool's answers to `asks` on a door served with `operator` rights or not.
    async fn ask_door(operator: bool, asks: &[serde_json::Value]) -> (serde_json::Value, Vec<serde_json::Value>) {
        let d = tempfile::tempdir().unwrap();
        let addr = crate::ipc::addr(d.path(), "cell-vault");
        let l = crate::ipc::Listener::bind(&addr).unwrap();
        let h = Handle::<Vault>::memory();
        h.store(|s| s.load(&crate::By::System, serde_json::json!({ "n": 1, "secret": "key" }))).unwrap();
        tokio::spawn(serve_with(h, l, Limits::default(), crate::link::no_calls(), operator, Default::default()));
        let mut s = crate::ipc::connect(&addr).await.unwrap();
        tokio::io::AsyncWriteExt::write_u8(&mut s, VERSION).await.unwrap();
        let mut f = frames(s);
        let text = |m: &serde_json::Value| Msg::Text(m.to_string());
        let next = async |f: &mut Frames, t: &str| loop {
            if let Some(Msg::Text(m)) = recv(f).await {
                let v: serde_json::Value = serde_json::from_str(&m).unwrap();
                if v["t"] == t {
                    return v;
                }
            }
        };
        send(&mut f, text(&serde_json::json!({ "t": "hello", "cell": "vault", "tool": true }))).await.unwrap();
        let first = next(&mut f, "state").await;
        let mut got = vec![];
        for a in asks {
            send(&mut f, text(a)).await.unwrap();
            got.push(next(&mut f, if a["t"] == "call" { "reply" } else { a["t"].as_str().unwrap() }).await);
        }
        (first, got)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_release_door_gives_a_client_s_rights_never_the_operator_s() {
        let asks = [
            serde_json::json!({ "t": "call", "method": "spend", "args": null }),
            serde_json::json!({ "t": "call", "method": "bump", "args": null }),
            serde_json::json!({ "t": "travel", "cmd": "undo" }),
            serde_json::json!({ "t": "load", "state": { "n": 9, "secret": "mine" } }),
            serde_json::json!({ "t": "clients" }),
        ];
        let (_, got) = ask_door(false, &asks).await;
        assert!(got[0]["err"].as_str().unwrap().contains("not a #[client] method"), "{}", got[0]);
        assert_eq!(got[1]["ok"], 2, "a #[client] method, as for a key holder");
        for g in &got[2..] {
            assert!(g["err"].as_str().unwrap().contains("need a debug build"), "{g}");
        }
        // A debug build's door: the operator's.
        let (_, got) = ask_door(true, &asks).await;
        assert!(got[0]["err"].is_null(), "{}", got[0]);
        assert!(got[2..].iter().all(|g| g["err"].is_null()), "{got:?}");
    }

}

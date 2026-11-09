//! An aio-rs app calling an aio app on this machine as itself, proven: aio's
//! `peer()` over the aio app's local socket (NDJSON, protocol v3; a named
//! pipe on Windows), its first frame aio's identity hello (`x-aio-peer`,
//! docs/migration/wire.md).
//!
//! aio checks a proof by finding the caller among its running instances
//! (its lock records) and reading the caller's per-run `control.key` from
//! that instance's home. So on its first use this app files both, as aio
//! does at boot ([`super::local`]: the same record and key `am` uses, one
//! per run); the app's stop (`aio::shutdown`) removes them. A crash leaves
//! a record aio drops as stale (its pid and start stamp no longer match)
//! and a key no proof verifies with.
//!
//! ```ignore
//! let Some(models) = AioPeer::find("models")? else { return Ok(()) }; // not running
//! let list = models.call("catalog", "list", json!([])).await?;
//! let r = models.call("queue", "ask", json!([{ "task": "chat" }, req])).await?;
//! let mut link = models.link().await?;  // one connection: state, many calls, leases
//! ```
use super::local;
use super::wire::AioLink;
use crate::Rejected;
use serde_json::Value;
use std::io;

/// A running aio app on this machine, called as this app.
#[derive(Clone, Debug)]
pub struct AioPeer {
    app: String,
}

impl AioPeer {
    /// The running aio app `app` (`None`: it isn't running), found by its
    /// lock record as aio's `peer(app)` finds it. Two running instances of
    /// it (two homes) are refused, naming both: guessing would call the
    /// wrong one's data.
    pub fn find(app: &str) -> io::Result<Option<Self>> {
        Ok(target(app)?.map(|_| Self { app: app.into() }))
    }

    /// Its id.
    pub fn app(&self) -> &str {
        &self.app
    }

    /// A link to it, proven as this app (aio's `resolvePeer` sees this app's
    /// id): its cells, live, and calls ([`AioLink`]). Found again on each
    /// link, so a restarted aio app is reached at its new boot.
    pub async fn link(&self) -> io::Result<AioLink> {
        let (me, key) = local::identity()?;
        let t = target(&self.app)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{} is not running", self.app)))?;
        let proof = proof(&key, &me, std::process::id(), &self.app, t.pid);
        AioLink::open(socket_wire(&t.socket).await?, Some(&proof)).await
    }

    /// Call `cell.method(args)` (a JSON list) in it, on a link of its own:
    /// its answer, or its refusal in its words. It waits as long as the aio
    /// app takes (a queued request may take minutes); a link that drops ends
    /// it, said as maybe run. Bound it with `tokio::time::timeout` if needed.
    pub async fn call(&self, cell: &str, method: &str, args: Value) -> crate::Result<Value> {
        let unreachable = |e: io::Error| Rejected::new(format!("{}: {cell}.{method}: {e}", self.app));
        self.link().await.map_err(unreachable)?.call_for(cell, method, args, None).await
    }
}

/// aio's peer MAC: hex HMAC-SHA256 under the caller's `control.key` (its
/// text) over who calls (`me`, `pid`) and which boot it calls (pure).
pub(crate) fn mac(key: &str, me: &str, pid: u32, target: &str, target_pid: u32) -> String {
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_bytes());
    let msg = format!("aio-peer/v1\n{me}\n{pid}\n{target}\n{target_pid}");
    crate::sha256::hex(ring::hmac::sign(&k, msg.as_bytes()).as_ref())
}

/// The `x-aio-peer` value: `v1 <app> <pid> <mac>` (pure).
fn proof(key: &str, me: &str, pid: u32, target: &str, target_pid: u32) -> String {
    format!("v1 {me} {pid} {}", mac(key, me, pid, target, target_pid))
}

/// An app id as aio takes one: `[A-Za-z0-9_.-]{1,200}` (pure).
fn app_id_ok(id: &str) -> bool {
    (1..=200).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

/// A lock record whose owner is gone: no pid, or not that process (its
/// start stamp, where the record has one and the system says it).
fn stale(rec: &Value) -> bool {
    let pid = rec["pid"].as_u64().and_then(|p| u32::try_from(p).ok()).filter(|p| *p > 0);
    pid.is_none_or(|p| {
        !crate::lifecycle::alive(p)
            || matches!((rec["startToken"].as_str(), local::start_token(p)), (Some(a), Some(b)) if a != b)
    })
}

/// The started instance of aio app `app` and where it listens.
struct Target {
    pid: u32,
    socket: String,
}

/// The ONE started instance of `app` (`None`: none), from aio's lock folder.
fn target(app: &str) -> io::Result<Option<Target>> {
    if !app_id_ok(app) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("not an aio app id: {app:?}")));
    }
    let dir = local::lock_dir()?;
    let mut up = vec![];
    for e in std::fs::read_dir(&dir)? {
        let name = e?.file_name().to_string_lossy().into_owned();
        let Some(key) = name.strip_suffix(".lock") else { continue };
        if key.rsplit_once('@').map_or(key, |(id, _)| id) != app {
            continue;
        }
        let Some(rec) = std::fs::read(dir.join(&name)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        else {
            continue;
        };
        if rec["appId"] == app && rec["status"] == "started" && rec.get("maintenance").is_none() && !stale(&rec) {
            up.push(rec);
        }
    }
    let rec = match up.len() {
        0 => return Ok(None),
        1 => up.remove(0),
        n => {
            let each = up.iter().map(|r| format!("pid {} home {}", r["pid"], r["home"])).collect::<Vec<_>>();
            return Err(io::Error::other(format!(
                "{app}: {n} instances are running ({}): stop the one you don't mean",
                each.join("; ")
            )));
        }
    };
    let pid = rec["pid"].as_u64().and_then(|p| u32::try_from(p).ok()).unwrap_or_default();
    match rec["socketPath"].as_str() {
        Some(s) => Ok(Some(Target { pid, socket: s.into() })),
        None => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "{app} listens on TCP port {} only; AioPeer reaches an aio app's local socket (aio's desktop default)",
                rec["port"]
            ),
        )),
    }
}

/// aio's local socket at `at` (a JSON message per line) as a link's frames.
async fn socket_wire(at: &str) -> io::Result<crate::sync::Wire> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Error, Message};
    use tokio_util::codec::{Framed, LinesCodec, LinesCodecError};
    let err = |e: LinesCodecError| match e {
        LinesCodecError::Io(e) => Error::Io(e),
        e => Error::Io(io::Error::other(e)),
    };
    let lines = Framed::new(crate::ipc::connect(at).await?, LinesCodec::new_with_max_length(MAX_LINE));
    let w = SinkExt::<String>::sink_map_err(lines, err)
        .with(|m: Message| -> futures_util::future::Ready<Result<String, Error>> {
            futures_util::future::ready(match m {
                Message::Text(t) => Ok(t.as_str().to_owned()),
                _ => Err(Error::Io(io::Error::other("aio's socket carries JSON text only"))),
            })
        })
        .map(move |r| r.map(Message::text).map_err(err));
    Ok(Box::new(w))
}

/// The longest line taken from aio's socket: a whole state (aio's own
/// limit is its `maxMessageBytes`).
const MAX_LINE: usize = 64 << 20;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_mac_is_aios() {
        // aio's `tag()` (server/peer-proof.ts), run in deno on these inputs.
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let want = "1d6b0e4d5785020cdbe1b93592cb473b35d9ccfa625e34a37f5b1c978685a511";
        assert_eq!(mac(key, "notes", 4242, "models", 3_896_373), want);
        assert_eq!(proof(key, "notes", 4242, "models", 3_896_373), format!("v1 notes 4242 {want}"));
        assert_ne!(mac(key, "notes", 4242, "models", 3_896_374), want, "bound to the target's boot");
    }

    #[test]
    fn app_ids_are_checked_as_aio_checks_them() {
        assert!(
            app_id_ok("models") && app_id_ok("a.b_c-1") && !app_id_ok("") && !app_id_ok("a/b") && !app_id_ok("a b")
        );
        assert!(!app_id_ok(&"a".repeat(201)) && app_id_ok(&"a".repeat(200)));
    }

    #[test]
    fn a_dead_or_reused_pid_is_stale() {
        assert!(!stale(&json!({ "pid": std::process::id() })));
        if cfg!(target_os = "linux") {
            let mine = local::start_token(std::process::id()).unwrap();
            assert!(!stale(&json!({ "pid": std::process::id(), "startToken": mine })));
            assert!(stale(&json!({ "pid": std::process::id(), "startToken": "1" })), "another process, same pid");
        }
        assert!(stale(&json!({ "pid": 0 })) && stale(&json!({})));
    }
}

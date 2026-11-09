//! One aio-rs app using another (aio's `peer()`), by its id: found on this
//! machine by where its data lives, reached through its local doors (no
//! port, no key: the OS admits only this user), and called as a client is,
//! never with the operator's rights its own tools get. What it lets a
//! client do (`#[client]` methods, their `allow` rules, `visible`, its
//! `Access::Open` server functions) is all a peer may.
//!
//! ```ignore
//! let Some(billing) = Peer::find("billing", "shop")? else { return Ok(()) }; // not running
//! billing.describe("ledger").await?;                    // what this caller may call
//! billing.describe_all().await?;                        // of every cell at once
//! let id = billing.call("ledger", "credit", json!([42])).await?;  // by name, no source
//! let ledger = billing.cell::<Ledger>().await?;         // typed: state, calls, server fns, streams
//! let receipt = billing.blobs().read(&id, 0, u64::MAX)?;  // its blobs, ranges too
//! ```
//!
//! Blobs are content-addressed files, whole under their id or absent, so a
//! peer reads and puts them in the other app's store directly. An app on
//! another machine is a paired server ([`crate::sync::Link`]): a `Remote`
//! calls it, and [`blob`] fetches its blob bytes over HTTP, with its key.
use crate::sync::{Link, Live, Remote};
use crate::{Cell, Rejected};
use serde_json::Value;
use std::path::PathBuf;

/// Another running aio app on this machine.
#[derive(Clone, Debug)]
pub struct Peer {
    app: String,
    data: PathBuf,
    user: String,
}

impl Peer {
    /// The running app `app` (`None`: it isn't running), called as `user`
    /// (this app's id: who its rules see; a claim, as a key holder's is).
    /// Its data is where its tools find it (`AIORT_DATA` followed).
    pub fn find(app: &str, user: &str) -> std::io::Result<Option<Self>> {
        Self::at(app, crate::paths::locate(app)?.0, user)
    }

    /// [`Self::find`] for the app `app` whose data is `data` (a profile, a
    /// ported aio home).
    pub fn at(app: &str, data: impl Into<PathBuf>, user: &str) -> std::io::Result<Option<Self>> {
        crate::paths::id_ok(app)?;
        let data = data.into();
        Ok(crate::paths::running(&data).then(|| Self { app: app.into(), data, user: user.into() }))
    }

    /// Its id.
    pub fn app(&self) -> &str {
        &self.app
    }

    fn door(&self, cell: &str) -> std::io::Result<Link> {
        let addr = crate::ipc::addr(&crate::paths::sockets_at(&self.app, &self.data), &format!("cell-{cell}"));
        Ok(Link { addr, token: String::new(), root: None })
    }

    /// Call `cell.method(args)` (a JSON list) in it, by name: its answer,
    /// or its refusal in its words. Not running (any more): refused at once,
    /// never queued (a service's answer is worth having now or not at all).
    pub async fn call(&self, cell: &str, method: &str, args: Value) -> crate::Result<Value> {
        let unreachable = |e: std::io::Error| Rejected::new(format!("{}: {cell}.{method}: {e}", self.app));
        let link = self.door(cell).map_err(unreachable)?;
        Live::peer(&link, cell, &self.user).await.map_err(unreachable)?.call(method, args).await
    }

    /// What this caller may call of `cell`: each method, its signature and
    /// whether its rule lets this caller in (`ax peer` shows the same).
    pub async fn describe(&self, cell: &str) -> std::io::Result<Value> {
        let mut live = Live::peer(&self.door(cell)?, cell, &self.user).await?;
        live.ask(serde_json::json!({ "t": "peer", "user": self.user })).await
    }

    /// [`Self::describe`] for every cell whose door it has open (aio's
    /// app-wide `peer().describe()`), by cell id.
    pub async fn describe_all(&self) -> std::io::Result<serde_json::Map<String, Value>> {
        let mut all = serde_json::Map::new();
        for cell in crate::door::open(&self.app, &self.data) {
            let d = self.describe(&cell).await?;
            all.insert(cell, d);
        }
        Ok(all)
    }

    /// A typed copy of its cell `T` (a crate both apps use): its state as
    /// this caller may see it, live; its methods, typed; its server
    /// functions and their streamed parts (`run`, `run_streamed`).
    pub async fn cell<T: Cell>(&self) -> std::io::Result<Remote<T>> {
        Remote::peer(self.door(T::ID)?, &self.user).await
    }

    /// Its blob store: read any blob (ranges too) and put bytes to pass to
    /// one of its methods by id.
    pub fn blobs(&self) -> crate::blobs::Blobs {
        crate::blobs::Blobs::at(self.data.join("files").join("blobs"))
    }
}

/// Bytes `start..end` of blob `id` of the app at `link` (`end` past its end:
/// to its end), over HTTP with its key (`GET /__aio/blobs/<id>`, pinned to
/// its root over TLS). Held to `[capabilities] network`.
#[cfg(feature = "outbound")]
pub async fn blob(link: &Link, id: &str, start: u64, end: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Error, ErrorKind};
    if !crate::blobs::id_ok(id) {
        return Err(Error::new(ErrorKind::InvalidInput, format!("blobs: {id:?} is not a blob id")));
    }
    if end <= start {
        return Ok(vec![]);
    }
    let (link, id) = (link.clone(), id.to_string());
    tokio::task::spawn_blocking(move || {
        let tls = match &link.root {
            Some(pem) => {
                let root = ureq::tls::Certificate::from_pem(pem.as_bytes()).map_err(Error::other)?;
                ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::new_with_certs(&[root])).build()
            }
            None => Default::default(),
        };
        let scheme = if link.root.is_some() { "https" } else { "http" };
        let agent = crate::capabilities::agent(
            "peer",
            ureq::Agent::config_builder().tls_config(tls).http_status_as_error(false).build(),
        );
        let url = format!("{scheme}://{}{}", link.addr, crate::blobs::url(&id));
        let last = if end == u64::MAX { String::new() } else { (end - 1).to_string() };
        let mut r = agent
            .get(&url)
            .header("authorization", format!("Bearer {}", link.token))
            .header("range", format!("bytes={start}-{last}"))
            .call()
            .map_err(|e| Error::other(format!("{url}: {e}")))?;
        match r.status().as_u16() {
            200 if start > 0 => Err(Error::other(format!("{url}: asked bytes {start}.. and got the whole blob"))),
            200 | 206 => r.body_mut().with_config().limit(u64::MAX).read_to_vec().map_err(Error::other),
            416 => Ok(vec![]),
            404 => Err(Error::new(ErrorKind::NotFound, format!("{url}: no such blob"))),
            s => Err(Error::new(
                if s == 401 { ErrorKind::PermissionDenied } else { ErrorKind::Other },
                format!("{url} answered {s}: {}", r.body_mut().read_to_string().unwrap_or_default()),
            )),
        }
    })
    .await
    .map_err(Error::other)?
}

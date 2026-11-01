//! How a copy stands with its server: the same for a native UI and a page.
/// How a UI's copy stands, for 'offline' and 'syncing…' indicators (aio
/// `useConnected`, `isConnectionDegraded`, `SyncStatus`). A cell in this
/// process is always online with nothing waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    /// Linked to the server now.
    pub online: bool,
    /// Calls the server hasn't confirmed yet.
    pub pending: usize,
    /// Calls back up: past 80% of the most that may wait (then calls are refused).
    pub degraded: bool,
    /// The link in one word.
    pub conn: Conn,
    /// When the server's state last reached the copy (ms since the epoch);
    /// `None` before it ever did in this run (started offline).
    pub last_sync: Option<i64>,
    /// The link's last round trip (aio's RTT probe); `None` in this process
    /// or before a ping is answered.
    pub rtt: Option<std::time::Duration>,
}

impl Status {
    /// A cell in this process.
    pub const LOCAL: Self =
        Self { online: true, pending: 0, degraded: false, conn: Conn::Online, last_sync: None, rtt: None };
}

/// The link in one word (aio's `SyncStatus.status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conn {
    /// Linked, nothing waiting.
    Online,
    /// No link: calls wait.
    Offline,
    /// Linked, calls on their way to the server.
    Syncing,
    /// So many calls wait that new ones are refused.
    Blocked,
}

impl Conn {
    /// Its name: `online`, `offline`, `syncing`, `blocked`.
    pub fn key(self) -> &'static str {
        ["online", "offline", "syncing", "blocked"][self as usize]
    }
}

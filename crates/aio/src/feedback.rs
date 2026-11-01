//! What a "Report a problem" UI needs (aio's `feedback` cell): send a report,
//! see it being sent, saved (and delivered) or why not, and how many are on
//! disk. The report is written as every problem report is (`aio::report`):
//! the app's cells, its calls and the bus, credentials masked; then sent on
//! to `[feedback] url` or the app's [`crate::report::sink`], never instead.
//! Nothing here is saved: the reports are the record.
use crate::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::RwLock;

/// Reports a user may send per [`WINDOW_MS`]: a person never meets it, a
/// script at once (each report writes the app's state to disk, and maybe out).
pub const MAX: usize = 5;
pub const WINDOW_MS: i64 = 60_000;

const NOT_SET_UP: &str = "reports are not set up for this app (aio::boot)";

/// Where the last report stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Idle,
    Capturing,
    Saved,
    Error,
}

/// What a sent report left.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub id: String,
    /// Where it was written: to attach it, or for a maintainer to find it.
    pub path: String,
    pub created_at: String,
    /// It also reached `[feedback] url` (or the app's sink).
    pub delivered: bool,
}

#[cell(id = "feedback")]
pub struct Feedback {
    /// Reports are set up (`ready`).
    pub enabled: bool,
    pub status: Status,
    /// The newest report this run: what a "thanks, saved" line reads.
    pub last: Option<Saved>,
    /// Reports on disk, the app's own included.
    pub pending: usize,
    pub error: Option<String>,
    /// When the reports of the last [`WINDOW_MS`] were sent.
    pub sent: Vec<i64>,
}

/// Write the report, send it on, count what is on disk (blocking).
fn capture(title: &str, body: &str, contact: Option<&str>) -> std::result::Result<(Saved, usize), String> {
    let p = crate::report::file_from(title, body, contact).map_err(|e| format!("the report was not saved: {e}"))?;
    let delivered = crate::report::send(&p).unwrap_or_else(|e| {
        eprintln!("[aio] report {} is saved but could not be delivered: {e}", p.display());
        false
    });
    let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let created_at = std::fs::read(&p)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|r| Some(r["createdAt"].as_str()?.to_string()))
        .unwrap_or_default();
    let n = p.parent().map_or(0, |d| crate::report::list(d).len());
    Ok((Saved { id, path: p.display().to_string(), created_at, delivered }, n))
}

fn count() -> std::result::Result<usize, String> {
    crate::report::about().map(|a| crate::report::list(&a.dir).len()).ok_or_else(|| NOT_SET_UP.to_string())
}

#[methods]
impl Feedback {
    /// Reports are set up, `pending` of them on disk (`open` does).
    pub fn ready(&mut self, pending: usize) {
        (self.enabled, self.pending) = (true, pending);
    }

    /// Send a report: `title` says the problem in a line; the facts are
    /// collected for you. Anyone using the app may: refusing them would
    /// silence the people who hit something; a budget of [`MAX`] a minute
    /// keeps it from filling the disk.
    #[client]
    pub fn report(&mut self, cx: &Cx, title: String, body: Option<String>, contact: Option<String>) -> Result {
        ensure!(self.enabled, "{NOT_SET_UP}");
        ensure!(!title.trim().is_empty(), "a report needs a one-line description of the problem");
        ensure!(self.status != Status::Capturing, "a report is being sent: wait for it");
        let now = cx.now();
        self.sent.retain(|t| now - t < WINDOW_MS);
        ensure!(
            self.sent.len() < MAX,
            "too many reports: this app takes {MAX} a minute, and they are spent; send it again in a minute"
        );
        self.sent.push(now);
        (self.status, self.error) = (Status::Capturing, None);
        cx.run(Effect::new("report", move |h: Handle<Feedback>| async move {
            let r = tokio::task::spawn_blocking(move || capture(&title, &body.unwrap_or_default(), contact.as_deref()))
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            _ = h.saved(r);
        }));
        Ok(())
    }

    /// A report's outcome, and how many are on disk now.
    pub fn saved(&mut self, r: std::result::Result<(Saved, usize), String>) {
        match r {
            Ok((s, n)) => (self.status, self.last, self.pending) = (Status::Saved, Some(s), n),
            Err(e) => (self.status, self.error) = (Status::Error, Some(e)),
        }
    }

    /// Count the reports on disk again.
    #[client]
    pub fn refresh(&mut self, cx: &Cx) -> Result {
        ensure!(self.enabled, "{NOT_SET_UP}");
        cx.run(
            Effect::new("refresh", |h: Handle<Feedback>| async move {
                let n = tokio::task::spawn_blocking(count).await.unwrap_or_else(|e| Err(e.to_string()));
                _ = h.counted(n);
            })
            .mode(Mode::First),
        );
        Ok(())
    }

    /// The count `refresh` found; a failure before it no longer stands.
    pub fn counted(&mut self, n: std::result::Result<usize, String>) {
        match n {
            Ok(n) if self.status == Status::Error => (self.pending, self.status, self.error) = (n, Status::Idle, None),
            Ok(n) => self.pending = n,
            Err(e) => (self.status, self.error) = (Status::Error, Some(e)),
        }
    }

    /// Clear the last report's banner.
    #[client]
    pub fn dismiss(&mut self) {
        if self.status != Status::Capturing {
            (self.status, self.last, self.error) = (Status::Idle, None, None);
        }
    }
}

static CELL: RwLock<Option<Handle<Feedback>>> = RwLock::new(None);

/// The app's `feedback` cell, once reports are set up (`aio::boot`).
pub fn cell() -> Option<Handle<Feedback>> {
    CELL.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Open the `feedback` cell on the reports [`crate::report::init`] set up.
pub fn open() -> Option<Handle<Feedback>> {
    let h = Handle::<Feedback>::memory();
    h.ready(count().ok()?).ok()?;
    *CELL.write().unwrap_or_else(|e| e.into_inner()) = Some(h.clone());
    Some(h)
}

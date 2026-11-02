//! The stand-in for [`presplash`](presplash.rs) without the `native-splash`
//! feature (an app with no `[build.splash]`, `ax dev`): it carries none of
//! the OS's splash; Electron's main process draws the splash (main.js), and
//! a download shows the download window.
use std::sync::Arc;

/// What draws the splash: nothing here.
pub(crate) enum Kind {}

pub(crate) fn kind(_: &str, _: &dyn Fn(&str) -> Option<String>) -> Option<Kind> {
    None
}

#[allow(dead_code)]
pub(crate) struct Plan<'a> {
    pub(crate) splash: &'a aio::config::Splash,
    pub(crate) title: &'a str,
    pub(crate) version: &'a str,
    pub(crate) colors: &'a [[String; 4]; 2],
    pub(crate) target: Option<crate::splash::Target>,
    pub(crate) at_start: bool,
    pub(crate) by_app: bool,
    pub(crate) may_download: bool,
}

pub(crate) struct Session;

impl Session {
    pub(crate) fn start(_: &Plan, k: Kind, _: &str) -> Option<Arc<Session>> {
        match k {}
    }
    pub(crate) fn download(&self) -> bool {
        false
    }
    pub(crate) fn progress(&self, _: Option<u64>, _: &str) -> crate::firstrun::Shown {
        crate::firstrun::Shown::Lost("no splash".into())
    }
    pub(crate) fn downloaded(&self) {}
    pub(crate) fn handoff(&self) -> bool {
        true
    }
}

pub(crate) fn current() -> Option<Arc<Session>> {
    None
}

pub(crate) enum Close {
    Failed,
}

pub(crate) fn end(_: Close) {}
pub(crate) fn ready(_: Option<[i32; 4]>) {}
pub(crate) fn moved(_: [i32; 4]) {}

pub(crate) struct Ends;

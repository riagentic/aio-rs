//! Where a page's view reads a cell: the page's own, or a synced copy.
use super::{Handle, sync::Remote};
use crate::{By, Cell, Door, Result, Status};
use serde_json::Value;
use std::sync::Arc;

pub enum Source<T: Cell> {
    Local(Handle<T>),
    Remote(Remote<T>),
}

impl<T: Cell> Clone for Source<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Local(h) => Self::Local(h.clone()),
            Self::Remote(r) => Self::Remote(r.clone()),
        }
    }
}

impl<T: Cell> Source<T> {
    /// The page's own cell of app `path` names, as the browser kept it
    /// ([`Handle::saved`]): what `aio::client!`'s `DB` opens in a page.
    pub fn open(path: &std::path::Path) -> std::result::Result<Self, String> {
        Ok(Self::Local(Handle::saved(&path.to_string_lossy())))
    }

    pub fn state(&self) -> Arc<T> {
        match self {
            Self::Local(h) => h.state(),
            Self::Remote(r) => r.state(),
        }
    }

    pub fn status(&self) -> Status {
        match self {
            Self::Local(_) => Status::LOCAL,
            Self::Remote(r) => r.status(),
        }
    }

    pub fn on_pending(&self, f: impl Fn() + Send + Sync + 'static) {
        match self {
            Self::Local(h) => h.on_pending(f),
            Self::Remote(_) => {}
        }
    }

    pub fn pending(&self, method: Option<&str>) -> usize {
        match self {
            Self::Local(h) => h.pending(method),
            Self::Remote(r) => method.map_or_else(|| r.pending(), |m| r.pending_of(m)),
        }
    }

    /// The views showed the state as it is now ([`Remote::rendered`]).
    pub fn rendered(&self) {
        if let Self::Remote(r) = self {
            r.rendered();
        }
    }

    pub fn listen(&self, f: impl Fn(&Arc<T>) + Send + Sync + 'static) {
        match self {
            Self::Local(h) => h.listen(f),
            Self::Remote(r) => r.listen(f),
        }
    }

    /// Put the page's own cell back to `state` (a dev build's Redux
    /// DevTools jump); a synced copy's history is its server's.
    pub(crate) fn put(&self, state: &Value) -> std::result::Result<(), String> {
        match self {
            Self::Local(h) => {
                h.put(serde_json::from_value(state.clone()).map_err(|e| format!("{}: {e}", T::ID))?);
                Ok(())
            }
            Self::Remote(_) => Err(format!("{}: a synced copy travels on its server (ax timetravel)", T::ID)),
        }
    }

    pub fn by(&self, by: By) -> Self {
        match self {
            Self::Local(h) => Self::Local(h.by(by)),
            Self::Remote(r) => Self::Remote(r.clone()),
        }
    }
}

impl<T: Cell> Door<T> for Source<T> {
    fn call(&self, method: &str, args: Value) -> Result<Value> {
        match self {
            Self::Local(h) => h.call(method, args),
            Self::Remote(r) => r.call(method, args),
        }
    }

    fn ask(&self, method: &str, args: Value) -> Result<crate::Answer> {
        match self {
            Self::Local(h) => h.ask(method, args),
            Self::Remote(r) => r.ask(method, args),
        }
    }

    fn read<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.state())
    }

    fn own(&self, key: &str, res: Option<Box<dyn std::any::Any + Send>>) {
        match self {
            Self::Local(h) => h.own(key, res),
            Self::Remote(r) => r.own(key, res),
        }
    }
}

//! What a method says to the people using the app (aio's `notify()` and
//! `appIcon()` effects): decided where the state changes, on the server
//! with no window in reach, and shown by every UI showing the cell. A
//! method asks with `cx.tell(..)`; after its commit the cell's own window
//! shows it (the desktop shell's [`show_with`]) and each synced client is
//! sent it, to show the same way. Nobody to show it to is said in the log.
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// One thing to tell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Tell {
    /// A desktop notification; one with the same `tag` replaces it, a click
    /// brings the app forward and goes to `route`.
    Notify {
        title: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        tag: Option<String>,
        #[serde(default)]
        silent: bool,
        #[serde(default)]
        route: Option<String>,
    },
    /// The app's icons and its tray's tooltip from now on, in one (aio's
    /// `appIcon()`): the `window`'s (and its taskbar entry's) and the
    /// `tray`'s, each PNG frames (base64 or `data:` URLs) looped at `fps`,
    /// at most 12; `Some([])`: the app's own again; `None`: as it is.
    /// `tooltip`: the tray's text when the pointer rests on it.
    Icon {
        #[serde(default)]
        window: Option<Vec<String>>,
        #[serde(default)]
        tray: Option<Vec<String>>,
        #[serde(default)]
        tooltip: Option<String>,
        fps: u8,
    },
}

impl Tell {
    /// A notification with just a title and a body.
    pub fn notify(title: &str, body: &str) -> Self {
        Tell::Notify { title: title.into(), body: body.into(), tag: None, silent: false, route: None }
    }

    /// Refused where it is asked: a blank title is a card nobody can read.
    pub fn check(&self) -> crate::Result {
        match self {
            Tell::Notify { title, .. } => crate::ensure!(!title.trim().is_empty(), "notify: the title is empty"),
            Tell::Icon { window, tray, tooltip, fps } => {
                crate::ensure!(window.is_some() || tray.is_some() || tooltip.is_some(), "icon: nothing to change");
                let moving = [window, tray].into_iter().flatten().any(|f| f.len() > 1);
                crate::ensure!(!moving || *fps > 0, "icon: frames need an fps")
            }
        }
        Ok(())
    }
}

pub(crate) type Show = Arc<dyn Fn(&Tell) + Send + Sync>;

static SHOW: RwLock<Option<Show>> = RwLock::new(None);

/// How this process shows a tell (set by the desktop shell).
pub fn show_with(f: impl Fn(&Tell) + Send + Sync + 'static) {
    *SHOW.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
}

/// Show `t` here; `false` with no way to.
pub fn show(t: &Tell) -> bool {
    let f = SHOW.read().unwrap_or_else(|e| e.into_inner()).clone();
    f.map(|f| f(t)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    #[cell]
    pub struct Job {
        done: bool,
    }

    #[methods]
    impl Job {
        pub fn finish(&mut self, cx: &Cx) -> Result {
            self.done = true;
            cx.tell(Tell::notify("Done", "the job"))
        }
    }

    #[test]
    fn a_standalone_cells_tell_is_shown_by_its_own_window() {
        let seen = Arc::new(std::sync::Mutex::new(vec![]));
        let s = seen.clone();
        show_with(move |t| s.lock().unwrap().push(t.clone()));
        let h = Handle::<Job>::memory();
        h.finish().unwrap();
        assert_eq!(*seen.lock().unwrap(), [Tell::notify("Done", "the job")]);
        assert!(Tell::notify("", "x").check().is_err());
        let icon = |window: Option<Vec<String>>, fps| Tell::Icon { window, tray: None, tooltip: None, fps };
        assert!(icon(Some(vec!["a".into(), "b".into()]), 0).check().unwrap_err().why.contains("need an fps"));
        assert!(icon(None, 1).check().unwrap_err().why.contains("nothing to change"));
        assert!(icon(Some(vec![]), 0).check().is_ok(), "the app's own again");
        assert_eq!(serde_json::to_value(Tell::notify("a", "b")).unwrap()["kind"], "notify");
    }
}

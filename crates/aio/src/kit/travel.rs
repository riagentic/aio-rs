//! The time-travel panel (aio's dev panel): Ctrl+. shows and hides it. It
//! lists the cell's newest steps (a click goes there), with undo, redo and
//! pause; the `client!`'s `use_time_travel()` feeds it and `time_travel`
//! takes its asks. Needs a [`GlobalKeys`](super::GlobalKeys) around the app.
//! A dev build mounts [`DevTimeTravel`] beside every app (aio's panel in
//! every dev app): each `client!` cell's history, on Ctrl+. too.
use super::keys::use_global_key;
use crate::ui::prelude::*;
use crate::{TimeAsk, Timeline};

/// What the rewinds are named in the log.
const REWIND: &str = "$rewind";

/// A `client!` cell's history and its steps (its `use_time_travel` and
/// `time_travel`), for [`DevTimeTravel`].
#[derive(Clone, Copy)]
#[doc(hidden)]
pub struct Travel {
    pub cell: &'static str,
    pub timeline: fn() -> Memo<Option<Timeline>>,
    pub ask: fn(TimeAsk) -> crate::Result<Timeline>,
}

/// What the views of this app instance said while drawing (never a signal:
/// drawing writes none): its `client!` cells, in the order they went live,
/// and whether it mounts its own [`TimeTravelPanel`].
#[derive(Clone, Default)]
struct Said(std::rc::Rc<std::cell::RefCell<(Vec<Travel>, bool)>>);

static SAID: Global<Said> = Global::named(Said::default, "aio::kit::travel::SAID");
/// [`DevTimeTravel`] shows.
static SHOWN: Global<bool> = Global::named(|| false, "aio::kit::travel::SHOWN");

/// Show or hide [`DevTimeTravel`] (what the page's Ctrl+. does).
pub fn toggle() {
    SHOWN.update(|s| *s = !*s);
}

/// `client!`'s `use_live()`: its cell's history reaches [`DevTimeTravel`]
/// (a dev build's: without `dev`, nothing, so a release carries none of it).
#[doc(hidden)]
pub fn register(t: Travel) {
    #[cfg(feature = "dev")]
    {
        let said = SAID.get_untracked();
        let mut s = said.0.borrow_mut();
        if !s.0.iter().any(|x| x.cell == t.cell) {
            s.0.push(t);
        }
    }
    #[cfg(not(feature = "dev"))]
    let _ = t;
}

/// The panel, hidden until Ctrl+. (or `open`); `timeline` `None` (a synced
/// copy) shows nothing.
#[component]
pub fn TimeTravelPanel(
    timeline: Prop<Option<Timeline>>,
    onask: EventHandler<TimeAsk>,
    #[props(default)] open: bool,
) -> View {
    SAID.get_untracked().0.borrow_mut().1 = true;
    let shown = signal(|| open);
    use_global_key("ctrl+.", move |_| shown.toggle());
    let tl = memo(move || timeline.get().filter(|_| shown.get()));
    rsx! { {tl.get().map(|tl| panel(tl, onask, 0))} }
}

/// What a dev build mounts beside the app (`aio_shell`'s window): every
/// `client!` cell's [`TimeTravelPanel`], side by side, hidden until Ctrl+.
/// (the page's key, so no [`GlobalKeys`](super::GlobalKeys) is needed); none
/// when the app mounts its own panel.
#[component]
pub fn DevTimeTravel() -> View {
    // Read as it shows: the app has drawn (and its cells gone live) by then.
    let cells = memo(move || {
        let said = SAID.get_untracked();
        let s = said.0.borrow();
        if SHOWN.get() && !s.1 { s.0.len() } else { 0 }
    });
    rsx! {
        // The page clicks it on Ctrl+. (`page::TRAVEL_KEY`).
        // The page clicks it on Ctrl+. (`page::TRAVEL_KEY`).
        div { "data-aio-travel": "", hidden: true, onclick: |_| toggle() }
        for at in 0..cells.get() {
            DevPanel { key: at, at }
        }
    }
}

/// The dev panel of the `at`th `client!` cell.
#[component]
fn DevPanel(at: usize) -> View {
    let t = SAID.get_untracked().0.borrow().0[at];
    let tl = (t.timeline)();
    let onask = callback(move |a| {
        if let Err(e) = (t.ask)(a) {
            eprintln!("[aio] time travel on {}: {}", t.cell, e.why);
        }
    });
    rsx! { {tl.get().map(|tl| panel(tl, onask, at))} }
}

/// The panel for one moment of the timeline, `nth` panels from the right.
fn panel(tl: Timeline, onask: EventHandler<TimeAsk>, nth: usize) -> View {
    // Nothing to undo at the start; nothing to redo unless an undo was last.
    let undo = tl.at > 0;
    let redo = tl.steps.last().is_some_and(|s| s.method == REWIND);
    let (ask, label) = if tl.paused { (TimeAsk::Resume, "Resume") } else { (TimeAsk::Pause, "Pause") };
    let steps: Vec<_> = tl.steps.iter().rev().map(|s| (s.seq, s.method.clone(), s.took)).collect();
    let (at, head, paused, right) = (tl.at, tl.head, tl.paused, 12 + nth * 292);
    rsx! {
        div {
            role: "region",
            "aria-label": "Time travel",
            "data-t": "timeTravel",
            style: "position:fixed;right:{right}px;top:12px;z-index:99999;width:280px;max-height:420px;overflow:auto;\
                    background:#f0f0f5;color:#333;border:1px solid #0002;border-radius:10px;font:12px/1.5 monospace;padding:8px",
            div { "data-t": "ttWhere",
                "time travel · at {at} of {head}"
                if paused { " · paused" }
            }
            div { style: "display:flex;gap:4px;margin:6px 0",
                button { r#type: "button", "data-t": "ttUndo", disabled: !undo, onclick: move |_| onask.call(TimeAsk::Undo), "Undo" }
                button { r#type: "button", "data-t": "ttRedo", disabled: !redo, onclick: move |_| onask.call(TimeAsk::Redo), "Redo" }
                button { r#type: "button", "data-t": "ttPause", onclick: move |_| onask.call(ask), "{label}" }
            }
            for (seq, method, took) in steps {
                button {
                    key: seq,
                    r#type: "button",
                    "data-t": "ttStep{seq}",
                    "aria-current": (seq == at).then_some("step"),
                    style: "display:block;width:100%;text-align:left;font:inherit",
                    onclick: move |_| onask.call(TimeAsk::Goto(seq)),
                    "#{seq} {method}"
                    {took.map(|d| format!(" {:.1} ms", d.as_secs_f64() * 1000.0))}
                }
            }
        }
    }
}

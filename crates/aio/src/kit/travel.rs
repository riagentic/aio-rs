//! The time-travel panel (aio's dev panel): Ctrl+. shows and hides it. It
//! lists the cell's newest steps (a click goes there), with undo, redo and
//! pause; the `client!`'s `use_time_travel()` feeds it and `time_travel`
//! takes its asks. Needs a [`GlobalKeys`](super::GlobalKeys) around the app.
use super::flag;
use super::keys::use_global_key;
use crate::ui::prelude::*;
use crate::{TimeAsk, Timeline};

/// What the rewinds are named in the log.
const REWIND: &str = "$rewind";

/// The panel, hidden until Ctrl+. (or `open`); `timeline` `None` (a synced
/// copy) shows nothing.
#[component]
pub fn TimeTravelPanel(timeline: Option<Timeline>, onask: EventHandler<TimeAsk>, #[props(default)] open: bool) -> View {
    let shown = signal(|| open);
    use_global_key("ctrl+.", move |_| shown.toggle());
    let Some(tl) = timeline.filter(|_| shown.get()) else { return rsx! {} };
    // Nothing to undo at the start; nothing to redo unless an undo was last.
    let undo = tl.at > 0;
    let redo = tl.steps.last().is_some_and(|s| s.method == REWIND);
    let (ask, label) = if tl.paused { (TimeAsk::Resume, "Resume") } else { (TimeAsk::Pause, "Pause") };
    rsx! {
        div {
            role: "region",
            "aria-label": "Time travel",
            "data-t": "timeTravel",
            style: "position:fixed;right:12px;top:12px;z-index:99999;width:280px;max-height:420px;overflow:auto;\
                    background:#f0f0f5;color:#333;border:1px solid #0002;border-radius:10px;font:12px/1.5 monospace;padding:8px",
            div { "data-t": "ttWhere",
                "time travel · at {tl.at} of {tl.head}"
                if tl.paused { " · paused" }
            }
            div { style: "display:flex;gap:4px;margin:6px 0",
                button { r#type: "button", "data-t": "ttUndo", disabled: flag(!undo), onclick: move |_| onask.call(TimeAsk::Undo), "Undo" }
                button { r#type: "button", "data-t": "ttRedo", disabled: flag(!redo), onclick: move |_| onask.call(TimeAsk::Redo), "Redo" }
                button { r#type: "button", "data-t": "ttPause", onclick: move |_| onask.call(ask), "{label}" }
            }
            for s in tl.steps.iter().rev() {
                button {
                    key: "{s.seq}",
                    r#type: "button",
                    "data-t": "ttStep{s.seq}",
                    "aria-current": (s.seq == tl.at).then_some("step"),
                    style: "display:block;width:100%;text-align:left;font:inherit",
                    onclick: {
                        let seq = s.seq;
                        move |_| onask.call(TimeAsk::Goto(seq))
                    },
                    "#{s.seq} {s.method}"
                    if let Some(d) = s.took { " {d.as_secs_f64() * 1000.0:.1} ms" }
                }
            }
        }
    }
}

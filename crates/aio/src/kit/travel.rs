//! The time-travel panel (aio's dev panel): Ctrl+. shows and hides it. It
//! lists the cell's newest steps (a click goes there), with undo, redo and
//! pause; the `client!`'s `use_time_travel()` feeds it and `time_travel`
//! takes its asks. Needs a [`GlobalKeys`](super::GlobalKeys) around the app.
use super::keys::use_global_key;
use crate::ui::prelude::*;
use crate::{TimeAsk, Timeline};

/// What the rewinds are named in the log.
const REWIND: &str = "$rewind";

/// The panel, hidden until Ctrl+. (or `open`); `timeline` `None` (a synced
/// copy) shows nothing.
#[component]
pub fn TimeTravelPanel(
    timeline: Prop<Option<Timeline>>,
    onask: EventHandler<TimeAsk>,
    #[props(default)] open: bool,
) -> View {
    let shown = signal(|| open);
    use_global_key("ctrl+.", move |_| shown.toggle());
    let tl = memo(move || timeline.get().filter(|_| shown.get()));
    rsx! { {tl.get().map(|tl| panel(tl, onask))} }
}

/// The panel for one moment of the timeline.
fn panel(tl: Timeline, onask: EventHandler<TimeAsk>) -> View {
    // Nothing to undo at the start; nothing to redo unless an undo was last.
    let undo = tl.at > 0;
    let redo = tl.steps.last().is_some_and(|s| s.method == REWIND);
    let (ask, label) = if tl.paused { (TimeAsk::Resume, "Resume") } else { (TimeAsk::Pause, "Pause") };
    let steps: Vec<_> = tl.steps.iter().rev().map(|s| (s.seq, s.method.clone(), s.took)).collect();
    let (at, head, paused) = (tl.at, tl.head, tl.paused);
    rsx! {
        div {
            role: "region",
            "aria-label": "Time travel",
            "data-t": "timeTravel",
            style: "position:fixed;right:12px;top:12px;z-index:99999;width:280px;max-height:420px;overflow:auto;\
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

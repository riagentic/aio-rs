//! The log viewer (amui's `ui/logs.tsx`): the chosen app's log tail, which
//! the manager reads again on demand and while following. A source
//! (combined, the app's log, its errors, its window's lines), a level floor,
//! a text filter, and lines coloured by level.
use super::app::{ViewState, pressed};
use super::manager::LogLine;
use super::proc::LogSource;
use aio::ui::prelude::*;

/// The lowest level shown (amui's level floor).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Floor {
    #[default]
    All,
    Info,
    Warn,
    Error,
}

fn rank(level: &str) -> u8 {
    match level {
        "trace" => 0,
        "debug" | "perf" => 1,
        "warn" => 3,
        "error" => 4,
        _ => 2,
    }
}

/// The lines a floor and a text keep (pure).
pub fn shown<'a>(all: &'a [LogLine], floor: Floor, q: &str) -> Vec<&'a LogLine> {
    let min = match floor {
        Floor::All => 0,
        Floor::Info => 2,
        Floor::Warn => 3,
        Floor::Error => 4,
    };
    let q = q.trim().to_lowercase();
    all.iter().filter(|l| rank(&l.level) >= min && (q.is_empty() || l.raw.to_lowercase().contains(&q))).collect()
}

const SOURCES: [(LogSource, &str, &str); 4] = [
    (LogSource::Combined, "Combined", "combined"),
    (LogSource::App, "Framework", "app"),
    (LogSource::Error, "Errors", "error"),
    (LogSource::Client, "Client", "client"),
];

const LEVELS: [(Floor, &str, &str); 4] = [
    (Floor::All, "all", "all"),
    (Floor::Info, "info+", "info"),
    (Floor::Warn, "warn+", "warn"),
    (Floor::Error, "error", "error"),
];

/// The viewer (amui's `LogView`): each prop follows its caller's state.
#[component]
pub fn LogView(
    logs: Prop<Option<Vec<LogLine>>>,
    loading: Prop<bool>,
    error: Prop<Option<String>>,
    source: Prop<LogSource>,
    path: Prop<Option<String>>,
    truncated: Prop<bool>,
    follow: Prop<bool>,
    on_reload: EventHandler<()>,
    on_source: EventHandler<LogSource>,
    on_follow: EventHandler<()>,
) -> View {
    let view = context::<ViewState>();
    let l = logs.clone();
    // The lines kept: they follow the log, the floor and the filter.
    let lines = memo(move || {
        l.with(|all| {
            let all = all.as_deref().unwrap_or(&[]);
            view.log_query.with(|q| shown(all, view.log_floor.get(), q).into_iter().cloned().collect::<Vec<LogLine>>())
        })
    });
    let total = memo(move || logs.with(|l| l.as_ref().map_or(0, Vec::len)));
    let pill = |on: bool| if on { "chip pill on" } else { "chip pill" };
    rsx! {
        div { class: "col10",
            div { style: "display:flex;gap:8px;align-items:center;flex-wrap:wrap",
                div { class: "row6", style: "gap:5px",
                    for (s , label , t) in SOURCES {
                        span { key: "{t}", class: pill(source.get() == s), role: "button", tabindex: "0", "data-t": "src-{t}",
                            "aria-pressed": "{source.get() == s}",
                            onclick: move |_| on_source.call(s),
                            onkeydown: move |e| if pressed(&e) { on_source.call(s) },
                            "{label}"
                        }
                    }
                }
                div { class: "row6", style: "gap:5px;margin-left:6px",
                    for (f , label , t) in LEVELS {
                        span { key: "{t}", class: pill(view.log_floor.get() == f), role: "button", tabindex: "0", "data-t": "lvl-{t}",
                            "aria-pressed": "{view.log_floor.get() == f}",
                            onclick: move |_| view.log_floor.set(f),
                            onkeydown: move |e| if pressed(&e) { view.log_floor.set(f) },
                            "{label}"
                        }
                    }
                }
                input { class: "field mono", style: "padding:5px 9px;font-size:12px;min-width:120px", "aria-label": "filter",
                    "data-t": "logFilter", placeholder: "filter…", value: view.log_query,
                    oninput: move |e| view.log_query.set(e.value()),
                }
                button { r#type: "button", class: "btn", "data-t": "logReload", disabled: loading.get(), title: "Read it again",
                    onclick: move |_| on_reload.call(()),
                    if loading.get() { "…" } else { "⟳" }
                }
                span { class: if follow.get() { "chip pill follow on" } else { "chip pill follow" }, role: "button", tabindex: "0",
                    "data-t": "follow", "aria-pressed": "{follow}",
                    onclick: move |_| on_follow.call(()),
                    onkeydown: move |e| if pressed(&e) { on_follow.call(()) },
                    if follow.get() { "● following" } else { "○ follow" }
                }
                span { class: "dim", style: "margin-left:auto;font-size:11px", "data-t": "logCount",
                    {format!("{}/{} lines", lines.with(Vec::len), total.get())}
                    if truncated.get() { " · tail" }
                }
            }
            if path.with(Option::is_some) {
                div { class: "small dim", style: "font-size:10px", "data-t": "logPath", {path.get().unwrap_or_default()} }
            }
            if error.with(Option::is_some) {
                div { class: "yellow", style: "font-size:13px;padding:8px 0", "data-t": "logError", {format!("ⓘ {}", error.get().unwrap_or_default())} }
            } else {
                div { class: "logbox", "data-t": "logLines",
                    if lines.with(Vec::is_empty) {
                        div { class: "dim", style: "padding:8px",
                            if total.get() > 0 { "no lines match the filter" } else { "no log lines" }
                        }
                    }
                    for (i , l) in lines.get().into_iter().enumerate() {
                        div { key: "{i}", class: "line log-{l.level}",
                            if l.level.is_empty() {
                                span { class: "text2", "{l.raw}" }
                            } else {
                                span { class: "dim", style: "flex-shrink:0", {l.ts.get(11..).unwrap_or(&l.ts).to_string()} }
                                span { class: "lvl l-{l.level}", {l.level.to_uppercase()} }
                                if !l.scope.is_empty() {
                                    span { class: "blue", style: "flex-shrink:0", "{l.scope}" }
                                }
                                span { class: "l-{l.level}", "{l.msg}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::manager::parse_log_line;

    /// amui's `parseLogLine: framework, client, and raw lines`.
    #[test]
    fn a_log_line_is_split_into_time_level_scope_and_text() {
        let l = parse_log_line("2026-11-01T10:56:46.280Z WARN  [page] [vitals] slow");
        assert_eq!(
            (l.ts.as_str(), l.level.as_str(), l.scope.as_str(), l.msg.as_str()),
            ("2026-11-01T10:56:46.280Z", "warn", "page", "[vitals] slow")
        );
        let i = parse_log_line("2026-11-01T10:56:44.914Z [aio] notes 0.1.0 starting");
        assert_eq!((i.level.as_str(), i.scope.as_str(), i.msg.as_str()), ("info", "aio", "notes 0.1.0 starting"));
        let cap = parse_log_line("[aio] boot app        notes (aio.toml)");
        assert_eq!((cap.ts.as_str(), cap.scope.as_str()), ("", "aio"), "a launch capture's line has no time");
        let raw = parse_log_line("thread 'main' panicked at src/lib.rs:3:5:");
        assert_eq!((raw.level.as_str(), raw.msg.as_str()), ("", raw.raw.as_str()), "shown as it is");
        let frame = parse_log_line("   0: std::backtrace");
        assert!(frame.level.is_empty());
    }

    /// amui's `LogView renders filtered lines`: the floor and the text.
    #[test]
    fn the_floor_and_the_text_pick_the_lines() {
        let all: Vec<LogLine> = [
            "2026-11-01T10:00:00.000Z DEBUG [x] noise",
            "2026-11-01T10:00:00.000Z [aio] up",
            "2026-11-01T10:00:00.000Z WARN  [ui] slow",
            "2026-11-01T10:00:00.000Z ERROR [aio] boom",
            "  a raw line",
        ]
        .iter()
        .map(|l| parse_log_line(l))
        .collect();
        assert_eq!(shown(&all, Floor::All, "").len(), 5);
        assert_eq!(shown(&all, Floor::Info, "").len(), 4, "a raw line reads as info: kept from info up");
        assert_eq!(shown(&all, Floor::Warn, "").len(), 2);
        assert_eq!(shown(&all, Floor::Error, "")[0].msg, "boom");
        assert_eq!(shown(&all, Floor::All, " SLOW ").len(), 1);
    }
}

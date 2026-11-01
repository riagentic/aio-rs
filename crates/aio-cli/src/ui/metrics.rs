//! The Metrics tab (amui's `ui/metrics.tsx`): everything the running app
//! measures, in one place. Trends (CPU, memory, its cells' state, the
//! slowest call's p95, pushes to clients), the calls its cells took (aio's
//! dispatch loop), its memory, who is linked, each cell's size, and its
//! newest events: how its state is actually changing.
use super::charts::AreaChart;
use super::manager::{Diag, Hist};
use aio::serde_json::Value;
use dioxus::prelude::*;

/// The colour of a fill: red from 80 %, yellow from 50 %.
fn gauge_color(pct: f64) -> &'static str {
    if pct >= 80.0 {
        "var(--red)"
    } else if pct >= 50.0 {
        "var(--yellow)"
    } else {
        "var(--green)"
    }
}

/// A labelled bar, current against its bound (amui's `Gauge`).
#[component]
fn Gauge(name: String, current: f64, capacity: f64, unit: String) -> Element {
    let pct = if capacity > 0.0 { (current / capacity * 100.0).clamp(0.0, 100.0) } else { 0.0 };
    let col = gauge_color(pct);
    let digits = if current < 10.0 { 2 } else { 0 };
    rsx! {
        div { class: "mb8",
            div { class: "small", style: "display:flex;justify-content:space-between;margin-bottom:3px",
                span { class: "text2", "{name}" }
                span { style: "color:{col}", "{current:.digits$} {unit} " span { class: "dim", "/ {capacity}" } }
            }
            div { class: "gauge", div { style: "width:{pct}%;height:100%;background:{col}" } }
        }
    }
}

#[component]
fn Stat(k: String, v: String, #[props(default)] color: String) -> Element {
    rsx! {
        div { class: "stat",
            span { "{k}" }
            span { style: if color.is_empty() { String::new() } else { format!("color:{color}") }, "{v}" }
        }
    }
}

#[component]
fn Card(title: String, t: String, children: Element) -> Element {
    rsx! {
        div { class: "card mcard", "data-t": "{t}",
            div { class: "mb8", style: "display:flex;justify-content:space-between;align-items:baseline",
                span { class: "label", "{title}" }
            }
            div { {children} }
        }
    }
}

fn num(v: &Value) -> f64 {
    v.as_f64().filter(|f| f.is_finite()).unwrap_or(0.0)
}

/// A time of day from ms since the epoch, this machine's zone.
pub fn clock(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms).single().map_or("—".into(), |t| t.format("%H:%M:%S").to_string())
}

/// A linked client's row: id, cell, user, transport, wire, round trip, since.
fn client_row(c: &Value) -> [String; 7] {
    let s = |k: &str| c[k].as_str().unwrap_or("—").to_string();
    [
        c["id"].to_string(),
        s("cell"),
        s("user"),
        s("from"),
        s("wire"),
        c["rtt"].as_u64().map_or("—".into(), |r| format!("{r} ms")),
        c["since"].as_i64().map_or("—".into(), clock),
    ]
}

/// An event's row: its seq, `cell.method`, its arguments, its time.
fn action_row(e: &Value) -> (String, String, String, String) {
    let s = |k: &str| e[k].as_str().unwrap_or("?").to_string();
    let at = e["at"].as_i64().map_or(String::new(), clock);
    (e["seq"].to_string(), format!("{}.{}", s("cell"), s("method")), e["args"].to_string(), at)
}

/// The window's mounted components as rows, each under its parent (pure):
/// depth, name, renders, the time they took (ms).
pub fn component_rows(tree: &Value) -> Vec<(usize, String, u64, f64)> {
    fn walk(v: &Value, depth: usize, out: &mut Vec<(usize, String, u64, f64)>) {
        for c in v.as_array().into_iter().flatten() {
            out.push((
                depth,
                c["name"].as_str().unwrap_or("?").into(),
                c["renders"].as_u64().unwrap_or(0),
                num(&c["ms"]),
            ));
            walk(&c["children"], depth + 1, out);
        }
    }
    let mut out = vec![];
    walk(&tree["components"], 0, &mut out);
    out
}

/// The panel (amui's `MetricsPanel`), all from props.
#[component]
pub fn MetricsPanel(diag: Diag, hist: Hist, connections: Option<u64>) -> Element {
    let cost = diag.cost.as_ref().and_then(Value::as_object).cloned().unwrap_or_default();
    let health = diag.health.clone().unwrap_or(Value::Null);
    let calls: f64 = cost.values().map(|c| num(&c["calls"])).sum();
    let mean = if calls > 0.0 {
        cost.values().map(|c| num(&c["meanMs"]) * num(&c["calls"])).sum::<f64>() / calls
    } else {
        0.0
    };
    let (p95, pushes) = diag.calls();
    let bytes_s: f64 = cost.values().map(|c| num(&c["bytesPerS"])).sum();
    let full: f64 = cost.values().map(|c| num(&c["fullResends"])).sum();
    let degraded: Vec<String> = health["degraded"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d["name"].as_str().map(String::from))
        .collect();
    let gauges: Vec<(String, f64, f64)> = health["gauges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| Some((g["name"].as_str()?.to_string(), num(&g["value"]), num(&g["bound"]))))
        .filter(|g| g.2 > 0.0)
        .collect();
    let last = diag
        .history
        .as_ref()
        .and_then(|h| h.last())
        .map(|e| format!("{}.{}", e["cell"].as_str().unwrap_or("?"), e["method"].as_str().unwrap_or("?")));
    let mem = &health["memory"];
    let rss = mem["rssBytes"].as_u64();
    let level = mem["level"].as_str().unwrap_or("—").to_string();
    let state = diag.state_bytes();
    let mut sizes: Vec<(String, u64)> = diag
        .metrics
        .as_ref()
        .and_then(|m| m["cells"].as_object())
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v["bytes"].as_u64().unwrap_or(0))).collect())
        .unwrap_or_default();
    sizes.sort_by_key(|s| std::cmp::Reverse(s.1));
    let max_cell = sizes.first().map_or(0, |s| s.1);
    let clients = diag.clients.clone().unwrap_or_default();
    let history: Vec<Value> = diag.history.clone().unwrap_or_default().into_iter().rev().collect();
    let comps = diag.components.as_ref().map(component_rows);
    let bytes = |n: f64| aio::fmt::bytes(n as u64);
    rsx! {
        div { class: "col",
            div { class: "wrap",
                AreaChart { title: "CPU", values: hist.cpu.clone(), unit: "%", color: "var(--green)" }
                AreaChart { title: "Memory (RSS)", values: hist.mem.clone(), unit: "MB", color: "var(--blue)" }
                AreaChart { title: "State", values: hist.state.clone(), unit: "KB", color: "var(--purple)" }
                AreaChart { title: "Call p95", values: hist.p95.clone(), unit: "ms", color: "var(--yellow)" }
                AreaChart { title: "Pushes", values: hist.pushes.clone(), unit: "/s", color: "var(--blue)" }
            }
            div { class: "wrap",
                Card { title: "calls", t: "calls",
                    if diag.cost.is_some() {
                        for (name , cur , cap) in gauges {
                            Gauge { key: "{name}", name: name.clone(), current: cur, capacity: cap, unit: "" }
                        }
                        Stat { k: "calls", v: format!("{calls:.0}") }
                        Stat { k: "mean call", v: format!("{mean:.2} ms") }
                        Stat { k: "p95 call", v: format!("{:.2} ms", p95.unwrap_or(0.0)) }
                        Stat { k: "pushes", v: format!("{:.1} /s", pushes.unwrap_or(0.0)) }
                        Stat { k: "sent", v: format!("{}/s", bytes(bytes_s)) }
                        Stat { k: "full resends", v: format!("{full:.0}"), color: if full > 0.0 { "var(--yellow)" } else { "" } }
                        Stat { k: "last action", v: last.unwrap_or("—".into()), color: "var(--blue)" }
                        Stat {
                            k: "degraded",
                            v: if degraded.is_empty() { "none".to_string() } else { degraded.join(", ") },
                            color: if degraded.is_empty() { "var(--green)" } else { "var(--red)" },
                        }
                    } else {
                        div { class: "dim", style: "font-size:12px", "no readings" }
                    }
                }
                Card { title: "memory", t: "memory",
                    if rss.is_some() || state.is_some() {
                        Stat { k: "rss", v: super::style::bytes(rss) }
                        Stat {
                            k: "of the machine",
                            v: format!("{}%", mem["machinePct"].as_u64().unwrap_or(0)),
                            color: gauge_color(mem["machinePct"].as_f64().unwrap_or(0.0)),
                        }
                        Stat {
                            k: "level",
                            v: level.clone(),
                            color: if level == "ok" { "var(--green)" } else { "var(--yellow)" },
                        }
                        Stat { k: "cells' state", v: super::style::bytes(state), color: "var(--purple)" }
                        Stat { k: "connections", v: connections.map_or("—".into(), |c| c.to_string()) }
                    } else {
                        div { class: "dim", style: "font-size:12px", "no memory readings" }
                    }
                }
            }
            Card { title: format!("clients ({})", clients.len()), t: "clients",
                if clients.is_empty() {
                    div { class: "dim", style: "font-size:12px", "no clients linked" }
                } else {
                    div { style: "overflow-x:auto",
                        table { class: "tbl",
                            thead {
                                tr {
                                    th { "id" } th { "cell" } th { "user" } th { "transport" } th { "wire" } th { "round trip" } th { "since" }
                                }
                            }
                            tbody {
                                for (i , row) in clients.iter().map(client_row).enumerate() {
                                    tr { key: "{i}",
                                        for (j , cell) in row.into_iter().enumerate() {
                                            td { key: "{j}", class: if j == 1 { "blue" } else if j == 6 { "dim" }, "{cell}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "wrap",
                Card { title: format!("cell state sizes ({})", sizes.len()), t: "cellSizes",
                    if sizes.is_empty() {
                        div { class: "dim", style: "font-size:12px", "no data" }
                    }
                    for (name , b , pct) in sizes.into_iter().map(|(n, b)| (n, b, if max_cell > 0 { b as f64 / max_cell as f64 * 100.0 } else { 0.0 })) {
                        div { key: "{name}", class: "mb6",
                            div { class: "small", style: "display:flex;justify-content:space-between;margin-bottom:2px",
                                span { class: "blue", "{name}" }
                                span { class: "text2", {aio::fmt::bytes(b)} }
                            }
                            div { class: "gauge", style: "height:4px",
                                div { style: "width:{pct:.1}%;height:100%;background:var(--blue-dim)" }
                            }
                        }
                    }
                }
                Card { title: format!("recent actions ({})", history.len()), t: "actions",
                    if history.is_empty() {
                        div { class: "dim", style: "font-size:12px", "no actions recorded yet" }
                    } else {
                        div { style: "max-height:260px;overflow-y:auto",
                            for (seq , what , args , at) in history.iter().map(action_row) {
                                div { key: "{seq}", class: "act-row",
                                    span { class: "text2 ell", title: "{args}", "{what}" }
                                    span { class: "dim", style: "flex-shrink:0", "{at}" }
                                }
                            }
                        }
                    }
                }
            }
            Card { title: format!("window components ({})", comps.as_ref().map_or(0, Vec::len)), t: "components",
                if comps.is_none() {
                    div { class: "dim", style: "font-size:12px", "no readings: a dev build keeps them (`ax components`)" }
                } else if comps.as_ref().is_some_and(Vec::is_empty) {
                    div { class: "dim", style: "font-size:12px", "no window mounted" }
                } else {
                    div { style: "max-height:260px;overflow-y:auto",
                        for (i , (depth , name , renders , ms)) in comps.unwrap_or_default().into_iter().enumerate() {
                            div { key: "{i}", class: "act-row",
                                span { class: "blue ell", style: "padding-left:{depth * 14}px", "{name}" }
                                span { class: "text2", style: "flex-shrink:0", "×{renders} · {ms:.2} ms" }
                            }
                        }
                    }
                }
            }
            div { class: "dim", style: "font-size:11px",
                "Sampled live every ~2.5 s while this app is selected · charts keep the last 60 samples."
            }
        }
    }
}

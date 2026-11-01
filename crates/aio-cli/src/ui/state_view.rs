//! Every cell's live state on one page (amui's `ui/state-view.tsx`), each
//! field badged with whether it is saved (survives a restart) and whether
//! it is sent to the window, and its JSON size beside it, as a bar against
//! its cell's largest field. A nested value opens into a JSON tree.
use super::app::{View, pressed};
use super::json_tree::{JsonTree, Shared};
use super::manager::{CellInfo, preview, size_of};
use aio::serde_json::Value;
use dioxus::prelude::*;
use std::rc::Rc;

/// On or off, said by more than its colour: an off badge is struck
/// through, and named "no disk" to a reader.
#[component]
pub fn Badge(on: bool, label: String, title: String) -> Element {
    let cls = if on { format!("badge {label} on") } else { format!("badge {label}") };
    rsx! {
        span { class: "{cls}", title: "{title}", role: "img",
            "aria-label": if on { label.clone() } else { format!("no {label}") },
            "{label}"
        }
    }
}

/// A slim bar against the cell's largest field, then the size.
#[component]
fn SizeBar(bytes: usize, max: usize, what: String) -> Element {
    let pct = (bytes * 100).checked_div(max).map_or(0, |p| p.max(2));
    let text = aio::fmt::bytes(bytes as u64);
    rsx! {
        span { title: "{what}: {text} as JSON — what the wire and the store carry",
            style: "margin-left:auto;display:inline-flex;align-items:center;gap:6px;flex-shrink:0",
            span { class: "bar", span { style: "width:{pct}%" } }
            span { class: "small dim", style: "min-width:60px;text-align:right", "{text}" }
        }
    }
}

/// A primitive's colour class.
fn tint(v: &Value) -> &'static str {
    match v {
        Value::Null => "dim",
        Value::String(_) => "green",
        Value::Number(_) => "blue",
        Value::Bool(_) => "purple",
        _ => "text2",
    }
}

#[component]
fn Field(cell: String, k: String, v: Shared, flags: (bool, bool), bytes: usize, max: usize) -> Element {
    let mut view = use_context::<View>();
    let obj = v.0.is_object() || v.0.is_array();
    let path = format!("{cell}.{k}");
    let open = obj && view.state_open.read().contains(&path);
    let (disk, ui) = flags;
    let p = path.clone();
    let mut toggle = move || {
        if obj {
            view.state_open.with_mut(|s| {
                if !s.remove(&p) {
                    s.insert(p.clone());
                }
            })
        }
    };
    let mut t2 = toggle.clone();
    rsx! {
        div { style: "border-top:1px solid var(--border)", "data-t": "field-{cell}-{k}",
            div { class: if obj { "frow obj" } else { "frow" },
                role: if obj { "button" },
                tabindex: if obj { "0" },
                onclick: move |_| toggle(),
                onkeydown: move |e| if pressed(&e) { t2() },
                span { style: "display:inline-flex;gap:3px;flex-shrink:0",
                    Badge { on: disk, label: "disk", title: if disk { "saved — survives a restart" } else { "not saved — in memory only" } }
                    Badge { on: ui, label: "ui", title: if ui { "sent to the window (synced to clients)" } else { "server only (#[server])" } }
                }
                span { class: "dim", style: "width:10px;flex-shrink:0", if obj { if open { "▾" } else { "▸" } } }
                span { style: "font-weight:600", "{k}" }
                span { class: "{tint(&v.0)} ell", style: "flex:1;min-width:0", if !open { "{preview(&v.0)}" } }
                SizeBar { bytes, max, what: k.clone() }
            }
            if open {
                div { style: "padding:0 10px 8px 40px",
                    JsonTree { data: v.clone(), prefix: path.clone() }
                }
            }
        }
    }
}

/// The merged state (amui's `StateOverview`): `state` is `ax snapshot`'s
/// object, `cells` what each cell says of its fields.
#[component]
pub fn StateOverview(state: Shared, cells: Vec<CellInfo>) -> Element {
    let Some(all) = state.0.as_object() else {
        return rsx! { div { class: "small dim", "no state" } };
    };
    let all: Vec<(String, Value)> = all.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    rsx! {
        div { class: "col", style: "gap:12px",
            div { class: "legend",
                span { "legend:" }
                Badge { on: true, label: "disk", title: "saved to the store" }
                span { "saved" }
                Badge { on: true, label: "ui", title: "sent to clients" }
                span { "sent to the window" }
            }
            for (cell , st) in all {
                {
                    let info = cells.iter().find(|c| c.name == cell).cloned().unwrap_or_default();
                    let fields: Vec<(String, Shared, usize)> = st
                        .as_object()
                        .map(|m| m.iter().map(|(k, v)| (k.clone(), Shared(Rc::new(v.clone())), size_of(v))).collect())
                        .unwrap_or_default();
                    let max = fields.iter().map(|f| f.2).max().unwrap_or(0);
                    let saved = fields.iter().filter(|f| info.flags(&f.0).0).count();
                    let sent = fields.iter().filter(|f| info.flags(&f.0).1).count();
                    let n = fields.len();
                    rsx! {
                        div { key: "{cell}", class: "cellstate", "data-t": "cellState-{cell}",
                            div { class: "cellstate-head",
                                span { class: "mono blue", style: "font-weight:700", "{cell}" }
                                span { class: "dim", style: "font-size:11px", "{n} fields · {saved} saved · {sent} in UI" }
                            }
                            if fields.is_empty() {
                                div { class: "dim", style: "padding:8px 12px;font-size:12px", "(empty)" }
                            }
                            for (k , v , bytes) in fields {
                                Field { key: "{k}", flags: info.flags(&k), cell: cell.clone(), k, v, bytes, max }
                            }
                        }
                    }
                }
            }
        }
    }
}

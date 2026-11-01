//! An expandable JSON tree (amui's `ui/json-tree.tsx`): objects and lists
//! fold, primitives are coloured by type, a folded container shows its
//! count. One set of open paths drives it; a node with thousands of
//! children shows its first [`CAP`] (a huge node would freeze the page).
use super::app::ViewState;
use aio::serde_json::Value;
use aio::ui::prelude::*;
use std::rc::Rc;

/// Children shown per node.
pub const CAP: usize = 200;

/// A value shared by the nodes that show its parts: equal when the same.
#[derive(Clone)]
pub struct Shared(pub Rc<Value>);

impl PartialEq for Shared {
    fn eq(&self, o: &Self) -> bool {
        Rc::ptr_eq(&self.0, &o.0)
    }
}

/// `k` as one step of a JSON pointer.
fn step(k: &str) -> String {
    format!("/{}", k.replace('~', "~0").replace('/', "~1"))
}

/// A primitive, coloured by its type (strings cut at 120).
fn primitive(v: &Value) -> View {
    match v {
        Value::Null => rsx! { span { class: "dim", "null" } },
        Value::String(s) if s.chars().count() > 120 => {
            let cut: String = s.chars().take(120).collect();
            rsx! { span { class: "green", "\"{cut}…\"" } }
        }
        Value::String(s) => rsx! { span { class: "green", "\"{s}\"" } },
        Value::Number(n) => rsx! { span { class: "blue", "{n}" } },
        Value::Bool(b) => rsx! { span { class: "purple", "{b}" } },
        o => rsx! { span { class: "text2", "{o}" } },
    }
}

#[component]
fn Node(root: Shared, ptr: String, k: String, path: String, depth: usize) -> View {
    let view = context::<ViewState>();
    let Some(v) = root.0.pointer(&ptr) else { return rsx! {} };
    let pad = format!("padding-left:{}px", depth * 14);
    if !(v.is_object() || v.is_array()) {
        return rsx! {
            div { class: "jrow", style: "{pad}",
                span { class: "text2", "{k}" }
                span { class: "dim", ": " }
                {primitive(v)}
            }
        };
    }
    let all = super::manager::entries(v);
    let (n, hidden) = (all.len(), all.len().saturating_sub(CAP));
    let open = view.tree_open.read().contains(&path);
    let (l, r) = if v.is_array() { ("[", "]") } else { ("{", "}") };
    let p = path.clone();
    let toggle = move || {
        view.tree_open.update(|s| {
            if !s.remove(&p) {
                s.insert(p.clone());
            }
        })
    };
    let t2 = toggle.clone();
    rsx! {
        div {
            div { class: "jrow", style: "{pad};cursor:pointer", role: "button", tabindex: "0",
                "aria-expanded": "{open}",
                onclick: move |_| toggle(),
                onkeydown: move |e| if super::app::pressed(&e) { t2() },
                span { class: "dim", style: "width:12px;display:inline-block", if open { "▾" } else { "▸" } }
                span { class: "text2", "{k}" }
                span { class: "dim", if open { ": {l}" } else { ": {l} {n} {r}" } }
            }
            if open {
                for (ck , _) in all.into_iter().take(CAP) {
                    Node {
                        key: "{ck}",
                        root: root.clone(),
                        ptr: format!("{ptr}{}", step(&ck)),
                        path: format!("{path}\u{0}{ck}"),
                        k: ck,
                        depth: depth + 1,
                    }
                }
                if hidden > 0 {
                    div { class: "jrow dim", style: "padding-left:{(depth + 1) * 14}px", "… {hidden} more (capped)" }
                }
                div { class: "jrow dim", style: "{pad}", "{r}" }
            }
        }
    }
}

/// `data`'s fields as a tree, its open paths under `prefix`.
#[component]
pub fn JsonTree(data: Shared, prefix: String) -> View {
    if !(data.0.is_object() || data.0.is_array()) {
        return rsx! { div { class: "small dim", "no state" } };
    }
    let keys: Vec<String> = super::manager::entries(&data.0).into_iter().map(|(k, _)| k).collect();
    rsx! {
        div { style: "overflow-x:auto", "data-t": "tree",
            for k in keys {
                Node { key: "{k}", root: data.clone(), ptr: step(&k), path: format!("{prefix}\u{0}{k}"), k, depth: 0 }
            }
        }
    }
}

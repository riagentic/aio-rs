//! `ax components <app>` (aio DevTools: `tree`, the per-component profile
//! and `renders`): the running app's mounted components, nested, each with
//! how many times it rendered and the time those renders took; with
//! `--renders`, its last renders as they happened. A dev build keeps them
//! (its render spans); its `ui` door answers `Aio.components`.
use crate::cdp::Session;
use aio::serde_json::{Value, json};

/// The tree, one component a line (pure): `List ×3 1.25 ms`, children
/// indented under their parent.
pub fn lines(tree: &Value) -> Vec<String> {
    fn walk(v: &Value, depth: usize, out: &mut Vec<String>) {
        for c in v.as_array().into_iter().flatten() {
            let (name, n, ms) =
                (c["name"].as_str().unwrap_or("?"), c["renders"].as_u64().unwrap_or(0), c["ms"].as_f64());
            out.push(format!("{}{name} ×{n} {:.2} ms", "  ".repeat(depth), ms.unwrap_or(0.0)));
            walk(&c["children"], depth + 1, out);
        }
    }
    let mut out = vec![];
    walk(tree, 0, &mut out);
    out
}

/// The last renders, one a line, then how many in all (pure):
/// `2026-10-29T10:00:00.000Z Row signal 0.05 ms`.
pub fn render_lines(renders: &Value, total: u64) -> Vec<String> {
    let mut out: Vec<String> = renders
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            format!(
                "{} {} {} {:.2} ms",
                aio::log::stamp(r["at"].as_i64().unwrap_or(0)),
                r["component"].as_str().unwrap_or("?"),
                r["trigger"].as_str().unwrap_or("?"),
                r["ms"].as_f64().unwrap_or(0.0)
            )
        })
        .collect();
    out.push(format!("{} of {total} renders", out.len()));
    out
}

/// Ask the window's door: the tree (or with `renders`, the last renders) as
/// lines, or all of it as JSON with `json`.
pub async fn run(s: &mut Session, json: bool, renders: bool) -> Result<String, String> {
    let r = s.call(aio_shell::inspect::COMPONENTS, json!({})).await.map_err(|e| {
        format!("{e} (the tree is a dev build's, through its own door: start it with `ax dev` or `ax start`, no --cdp)")
    })?;
    if json {
        return Ok(r.to_string());
    }
    if renders {
        return Ok(render_lines(&r["renders"], r["total"].as_u64().unwrap_or(0)).join("\n"));
    }
    let l = lines(&r["components"]);
    Ok(if l.is_empty() { "no components mounted yet".into() } else { l.join("\n") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_is_one_component_a_line_nested_with_its_renders_and_time() {
        let row = |n| json!({ "name": "Row", "renders": n, "ms": 0.5, "children": [] });
        let tree = json!([{ "name": "root", "renders": 2, "ms": 1.234, "children": [
            { "name": "List", "renders": 1, "ms": 0.2, "children": [row(1), row(3)] }
        ] }]);
        assert_eq!(lines(&tree), ["root ×2 1.23 ms", "  List ×1 0.20 ms", "    Row ×1 0.50 ms", "    Row ×3 0.50 ms"]);
        assert!(lines(&json!([])).is_empty());
        let r = json!([{ "component": "Row", "at": 0, "ms": 0.051, "trigger": "signal" }]);
        assert_eq!(render_lines(&r, 7), ["1970-01-01T00:00:00.000Z Row signal 0.05 ms", "1 of 7 renders"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_windows_door_answers_with_its_tree() {
        let tree = json!([{ "name": "root", "renders": 1, "ms": 0.1, "children": [] }]);
        let all = json!({ "components": tree, "renders": [], "total": 1 });
        let answers = crate::cdp::tests::Answers::from([(aio_shell::inspect::COMPONENTS.to_string(), all.clone())]);
        let (at, _d) = crate::cdp::tests::door_endpoint(answers).await;
        let mut s = crate::cdp::window(&at, 0).await.unwrap();
        assert_eq!(run(&mut s, false, false).await.unwrap(), "root ×1 0.10 ms");
        assert_eq!(run(&mut s, false, true).await.unwrap(), "0 of 1 renders");
        assert_eq!(run(&mut s, true, false).await.unwrap(), all.to_string());
    }
}

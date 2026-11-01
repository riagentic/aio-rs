//! `ax eval <app> <code>` (aio's `am eval`): a JavaScript expression, or
//! statements (the last one's value, as the DevTools console gives it),
//! evaluated in the app's live window, its value as JSON, over the DevTools
//! protocol (as `ax shot`). For what `ax surface` can't
//! see: geometry, computed styles, a fetch from the page's own origin. A
//! promise is awaited; a throw is a failure, never a value.
use crate::cdp::Session;
use aio::serde_json::{Value, json};

/// What [`wrap`] puts before the expression (columns are the user's).
const PREFIX: &str = "Promise.resolve((";

/// A value made JSON: a DOM node summarised (it would be `{}`), a DOMRect
/// by its `toJSON`, a circular value named, not thrown.
const JSONED: &str = "function(v){\
     if(v===undefined)return undefined;\
     if(v&&typeof v===\"object\"&&typeof v.nodeType===\"number\")\
     return{node:v.nodeName,id:v.id||undefined,class:v.className||undefined,\
     rect:v.getBoundingClientRect?JSON.parse(JSON.stringify(v.getBoundingClientRect())):undefined,\
     text:(v.textContent||\"\").slice(0,200)};\
     try{return JSON.parse(JSON.stringify(v))}catch(_){return{__aio_unserializable:String(v)}}}";

/// The expression as it runs (pure): in parens (`{ a: 1 }` is an object,
/// not a block), awaited, then made JSON ([`JSONED`]).
pub fn wrap(expr: &str) -> String {
    format!("{PREFIX}{expr})).then({JSONED})")
}

/// `Runtime.evaluate`'s answer as a value and its type (`undefined` stays
/// apart from `null`), or why it threw, at the user's line and column (pure).
pub fn outcome(reply: &Value) -> Result<(Value, String), String> {
    outcome_after(reply, PREFIX.len())
}

/// [`outcome`] of code that ran after `prefix` columns of aio's own.
fn outcome_after(reply: &Value, prefix: usize) -> Result<(Value, String), String> {
    if let Some(ex) = reply.get("exceptionDetails") {
        let e = &ex["exception"];
        let why = e["description"]
            .as_str()
            .map(String::from)
            .or_else(|| e.get("value").filter(|v| !v.is_null()).map(|v| v.as_str().map_or(v.to_string(), String::from)))
            .or_else(|| ex["text"].as_str().map(String::from))
            .unwrap_or_else(|| "it threw".into());
        let at = match (ex["lineNumber"].as_i64(), ex["columnNumber"].as_i64()) {
            (Some(l), c) => {
                let c = c.unwrap_or(0) - if l == 0 { prefix as i64 } else { 0 };
                format!(" (at {l}:{})", c.max(0))
            }
            (None, _) => String::new(),
        };
        return Err(format!("{why}{at}"));
    }
    let r = &reply["result"];
    Ok((r.get("value").cloned().unwrap_or(Value::Null), r["type"].as_str().unwrap_or("undefined").into()))
}

/// `code` in the page `s` is a session with: an expression as [`wrap`]
/// has it; else (it doesn't parse as one: `const f = …; f(2)`) statements,
/// run as the DevTools console runs them (the last one's value, top-level
/// `await`), that value then made JSON.
pub async fn evaluate(s: &mut Session, code: &str) -> Result<(Value, String), String> {
    let threw = |e: String| format!("{code} threw: {e}");
    // Parsing alone (`compileScript`) needs the session's Runtime on.
    s.call("Runtime.enable", json!({})).await?;
    let compiled =
        s.call("Runtime.compileScript", json!({ "expression": wrap(code), "sourceURL": "", "persistScript": false }));
    if compiled.await?.get("exceptionDetails").is_none() {
        let r = s
            .call("Runtime.evaluate", json!({ "expression": wrap(code), "awaitPromise": true, "returnByValue": true }))
            .await?;
        return outcome(&r).map_err(threw);
    }
    let r = s.call("Runtime.evaluate", json!({ "expression": code, "replMode": true, "awaitPromise": true })).await?;
    let done = outcome_after(&r, 0).map_err(threw)?;
    let Some(id) = r["result"]["objectId"].as_str() else { return Ok(done) };
    let f = format!("function(){{return ({JSONED})(this)}}");
    let r = s
        .call("Runtime.callFunctionOn", json!({ "objectId": id, "functionDeclaration": f, "returnByValue": true }))
        .await?;
    outcome_after(&r, 0).map_err(threw)
}

/// `expr` in the app's window `window` reached `at`: its value as JSON text,
/// or `undefined`.
pub fn run(at: &crate::cdp::Reach, window: usize, expr: &str) -> Result<String, String> {
    let (v, ty) = aio::rt().block_on(async { evaluate(&mut crate::cdp::window(at, window).await?, expr).await })?;
    Ok(if ty == "undefined" { ty } else { aio::serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_throw_is_a_failure_at_the_users_column_and_undefined_is_not_null() {
        let r = |v: Value| outcome(&v);
        assert_eq!(r(json!({ "result": { "type": "number", "value": 2 } })), Ok((json!(2), "number".into())));
        assert_eq!(r(json!({ "result": { "type": "undefined" } })), Ok((Value::Null, "undefined".into())));
        assert_eq!(r(json!({ "result": { "type": "object", "value": null } })), Ok((Value::Null, "object".into())));
        let thrown = json!({ "exceptionDetails": { "text": "Uncaught", "lineNumber": 0, "columnNumber": 20,
            "exception": { "description": "Error: boom" } } });
        assert_eq!(r(thrown), Err(format!("Error: boom (at 0:{})", 20 - PREFIX.len())));
        let value = json!({ "exceptionDetails": { "text": "Uncaught", "exception": { "value": "plain" } } });
        assert_eq!(r(value), Err("plain".into()), "a thrown string");
        // Statements run as typed: their columns are the user's own.
        let at = json!({ "exceptionDetails": { "exception": { "description": "SyntaxError: x" },
            "lineNumber": 0, "columnNumber": 3 } });
        assert_eq!(outcome_after(&at, 0), Err("SyntaxError: x (at 0:3)".into()));
    }

    #[test]
    fn an_expression_is_evaluated_in_the_apps_window() {
        let answers = crate::cdp::tests::Answers::from([
            ("Runtime.enable".into(), json!({})),
            ("Runtime.compileScript".into(), json!({})),
            ("Runtime.evaluate".into(), json!({ "result": { "type": "object", "value": { "a": 1 } } })),
        ]);
        let port = &crate::cdp::Reach::Port(aio::rt().block_on(crate::cdp::tests::endpoint(answers)));
        assert_eq!(run(port, 0, "({ a: 1 })"), Ok("{\n  \"a\": 1\n}".into()));
        assert!(run(port, 1, "1").unwrap_err().contains("no window 1: the app has 1 (0)"));
    }

    /// aio am-eval-cdp.test.ts, against a real Chromium (skipped without one).
    #[cfg(unix)]
    #[test]
    fn a_real_page_answers_geometry_promises_objects_nodes_and_throws() {
        let Some(page) = crate::cdp::tests::Chromium::open("<div id=r style='width:50px;height:20px'>hello</div>")
        else {
            return;
        };
        aio::rt().block_on(async {
            let mut s = page.session().await;
            let mut ev = async |e: &str| evaluate(&mut s, e).await;
            let rect = ev("document.getElementById('r').getBoundingClientRect()").await.unwrap().0;
            assert_eq!((rect["width"].as_f64(), rect["height"].as_f64()), (Some(50.0), Some(20.0)), "{rect}");
            assert_eq!(ev("Promise.resolve(42)").await.unwrap().0, 42, "awaited, not {{}}");
            assert_eq!(ev("{ a: 1 }").await.unwrap().0, json!({ "a": 1 }), "an object, not a block");
            let node = ev("document.getElementById('r')").await.unwrap().0;
            assert_eq!(
                (node["node"].as_str(), node["id"].as_str(), node["text"].as_str()),
                (Some("DIV"), Some("r"), Some("hello"))
            );
            assert_eq!(node["rect"]["width"], 50);
            for e in ["(() => { throw new Error('boom') })()", "Promise.reject(new Error('boom'))"] {
                assert!(ev(e).await.unwrap_err().contains("boom"), "{e} is a throw, not a value");
            }
            // Statements: the last one's value, awaited, made JSON as an expression's.
            assert_eq!(ev("1; 2").await.unwrap().0, 2);
            assert_eq!(ev("const f = (x) => x * 2; f(21)").await.unwrap().0, 42);
            assert_eq!(ev("let n = await Promise.resolve(5); n + 1").await.unwrap().0, 6);
            assert_eq!(ev("const d = document.getElementById('r'); d").await.unwrap().0["node"], "DIV");
            assert_eq!(ev("let u; u").await.unwrap().1, "undefined");
            assert!(ev("let z = 1; throw new Error('late')").await.unwrap_err().contains("late"));
            let syntax = ev("1 +").await.unwrap_err();
            assert!(syntax.contains("SyntaxError"), "{syntax}");
            assert_eq!(ev("undefined").await.unwrap().1, "undefined");
            assert_eq!(ev("null").await.unwrap(), (Value::Null, "object".into()));
            let circular = ev("(() => { const o = {}; o.o = o; return o })()").await.unwrap().0;
            assert!(circular.get("__aio_unserializable").is_some(), "{circular}");
        });
    }
}

//! The page's document for the views (`dioxus_document`: the title, head
//! tags, `document::eval`), written through its DOM, never through `eval`
//! (the page's policy has none): code from the app runs as a script the
//! page makes (`blob:`), as on the desktop page (`aio::page::RUN`).
use dioxus_core::internal::generational_box::{AnyStorage, Owner, UnsyncStorage};
use dioxus_document::{Document, Eval, EvalError, Evaluator};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

// One script run: its body in an async function given `dioxus` (`send`,
// `recv`), its result, and its messages both ways as JSON text.
#[wasm_bindgen(inline_js = r#"
let n = 0;
const runs = (globalThis.__aioRuns = globalThis.__aioRuns || {});
const take = (q, w) => new Promise((r) => (q.length ? r(q.shift()) : w.push(r)));
const put = (q, w, v) => (w.length ? w.shift()(v) : q.push(v));
const text = (v) => JSON.stringify(v === undefined ? null : v);
export function run(code) {
  const id = ++n, inbox = [], waiting = [], out = [], asked = [];
  let done;
  const result = new Promise((ok, no) => { done = { ok, no }; });
  const end = (f, v) => { if (runs[id]) { delete runs[id]; f(v); } };
  runs[id] = {
    dioxus: { send: (v) => put(out, asked, text(v)), recv: () => take(inbox, waiting), close: () => {} },
    ok: (v) => end(done.ok, text(v)),
    no: (e) => end(done.no, String((e && e.stack) || e)),
  };
  const r = `__aioRuns[${id}]`;
  const src = `${r}.started = 1; (async (dioxus) => {\n${code}\n})(${r}.dioxus).then((v) => ${r}.ok(v), (e) => ${r}.no(e));`;
  const u = URL.createObjectURL(new Blob([src], { type: 'text/javascript' }));
  const s = document.createElement('script');
  const gone = () => { URL.revokeObjectURL(u); s.remove(); };
  s.src = u;
  s.onload = () => { gone(); if (runs[id] && !runs[id].started) runs[id].no('the script did not parse'); };
  s.onerror = () => { gone(); if (runs[id]) runs[id].no('the page policy refused the script'); };
  document.head.appendChild(s);
  return { result, send: (j) => put(inbox, waiting, JSON.parse(j)), next: () => take(out, asked) };
}
"#)]
extern "C" {
    type Run;
    fn run(code: &str) -> Run;
    #[wasm_bindgen(method, getter)]
    fn result(this: &Run) -> js_sys::Promise;
    #[wasm_bindgen(method)]
    fn send(this: &Run, json: &str);
    #[wasm_bindgen(method)]
    fn next(this: &Run) -> js_sys::Promise;
}

/// The page's document ([`super::launch`] gives it to the views).
pub(crate) struct Page;

thread_local! {
    /// Holds the scripts started outside any component (at an unmount).
    static LOOSE: Owner<UnsyncStorage> = UnsyncStorage::owner();
}

impl Document for Page {
    fn eval(&self, js: String) -> Eval {
        let r = run(&js);
        let script = Script { result: JsFuture::from(r.result()), next: None, run: r };
        // The component that asked owns it: it goes with the component.
        let in_scope = dioxus_core::Runtime::try_current().and_then(|rt| rt.try_current_scope_id()).is_some();
        let owner = if in_scope { dioxus_core::current_owner::<UnsyncStorage>() } else { LOOSE.with(Clone::clone) };
        Eval::new(owner.insert(Box::new(script) as Box<dyn Evaluator>))
    }

    fn set_title(&self, title: String) {
        if let Some(d) = web_sys::window().and_then(|w| w.document()) {
            d.set_title(&title);
        }
    }

    fn create_head_element(&self, name: &str, attributes: &[(&str, String)], contents: Option<String>) {
        let Some(d) = web_sys::window().and_then(|w| w.document()) else { return };
        let (Some(head), Ok(e)) = (d.head(), d.create_element(name)) else { return };
        attributes.iter().for_each(|(k, v)| _ = e.set_attribute(k, v));
        match contents {
            // An inline script would be refused: its text runs as the page's own.
            Some(c) if name == "script" => _ = run(&c),
            Some(c) => e.set_text_content(Some(&c)),
            None => {}
        }
        _ = head.append_child(&e);
    }
}

/// A script running in the page.
struct Script {
    run: Run,
    result: JsFuture,
    next: Option<JsFuture>,
}

/// A JSON text from the page, or why there is none.
fn json(r: Result<JsValue, JsValue>) -> Result<serde_json::Value, EvalError> {
    let text = r.map_err(|e| EvalError::Communication(e.as_string().unwrap_or_else(|| format!("{e:?}"))))?;
    serde_json::from_str(&text.as_string().unwrap_or_default()).map_err(|e| EvalError::Communication(e.to_string()))
}

impl Evaluator for Script {
    fn send(&self, data: serde_json::Value) -> Result<(), EvalError> {
        self.run.send(&data.to_string());
        Ok(())
    }

    fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Result<serde_json::Value, EvalError>> {
        let next = self.next.get_or_insert_with(|| JsFuture::from(self.run.next()));
        let got = Pin::new(next).poll(cx);
        if got.is_ready() {
            self.next = None;
        }
        got.map(json)
    }

    fn poll_join(&mut self, cx: &mut Context<'_>) -> Poll<Result<serde_json::Value, EvalError>> {
        Pin::new(&mut self.result).poll(cx).map(json)
    }
}

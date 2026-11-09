//! The chunks of a split web build (`aio::lazy!`, `ax build --web`): a
//! lazy view's code, fetched and instantiated the first time it shows,
//! once per page (and per worker). A build that was not split (a dev
//! build, or a view with nothing of its own) has every view in its one
//! module: nothing is fetched.
use js_sys::{Object, Promise, Reflect, WebAssembly};
use std::cell::RefCell;
use std::collections::BTreeMap;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// The main module's chunks: its build's hash, and for each lazy entry
/// (its table slot) with a chunk whether it needs the common one.
type Plan = Option<(String, BTreeMap<u32, bool>)>;

thread_local! {
    static PLAN: std::cell::OnceCell<Plan> = const { std::cell::OnceCell::new() };
    /// Each chunk's instance (a promise of it), by file.
    static CHUNKS: RefCell<BTreeMap<String, Promise>> = RefCell::default();
    /// The entries whose chunks this thread has.
    static LOADED: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
}

/// The plan, from the main module's export names (`$m<hash>`, `aio-lazy-<slot>[-c]`).
fn plan() -> Plan {
    PLAN.with(|p| {
        p.get_or_init(|| {
            let names = Object::keys(wasm_bindgen::exports().unchecked_ref::<Object>());
            let names: Vec<String> = names.iter().filter_map(|n| n.as_string()).collect();
            let hash = names.iter().find_map(|n| n.strip_prefix("$m"))?.to_string();
            let entries = names
                .iter()
                .filter_map(|n| n.strip_prefix("aio-lazy-"))
                .filter_map(|n| match n.strip_suffix("-c") {
                    Some(s) => Some((s.parse().ok()?, true)),
                    None => Some((n.parse().ok()?, false)),
                })
                .collect();
            Some((hash, entries))
        })
        .clone()
    })
}

/// Where the site's files are: its `start.js`'s (or `worker.js`'s) folder.
pub(crate) fn base() -> String {
    Reflect::get(&js_sys::global(), &"__aioBase".into()).ok().and_then(|b| b.as_string()).unwrap_or("./".into())
}

/// The entries whose chunks this thread has (a worker loads them too).
pub(crate) fn loaded() -> Vec<u32> {
    LOADED.with_borrow(Clone::clone)
}

/// Make sure lazy entry `slot`'s code is here: its chunk (and the common
/// one, if it needs it) fetched and instantiated, once. A failure is
/// said, and the next try fetches again.
pub(crate) async fn load(slot: u32) -> Result<(), String> {
    let Some((hash, entries)) = plan() else { return Ok(()) };
    let Some(&common) = entries.get(&slot) else { return Ok(()) };
    let c = common.then(|| chunk(crate::web::chunk_file(&hash, None), None));
    let own = chunk(crate::web::chunk_file(&hash, Some(slot)), c.clone().map(|c| c.1));
    let files: Vec<(String, Promise)> = c.into_iter().chain([own]).collect();
    for (file, p) in &files {
        if let Err(e) = JsFuture::from(p.clone()).await {
            // Each is fetched again on the next try (the view's waits on the common one).
            CHUNKS.with_borrow_mut(|m| files.iter().for_each(|(f, _)| _ = m.remove(f)));
            return Err(format!("the view's code ({file}) could not be loaded: {}", said(&e)));
        }
    }
    LOADED.with_borrow_mut(|l| {
        if !l.contains(&slot) {
            l.push(slot)
        }
    });
    Ok(())
}

fn said(e: &JsValue) -> String {
    e.as_string().or_else(|| e.dyn_ref::<js_sys::Error>().map(|e| e.message().into())).unwrap_or(format!("{e:?}"))
}

/// The instance of chunk `file` (a promise), started once.
fn chunk(file: String, common: Option<Promise>) -> (String, Promise) {
    let p = CHUNKS.with_borrow_mut(|m| {
        m.entry(file.clone())
            .or_insert_with(|| wasm_bindgen_futures::future_to_promise(instance(file.clone(), common)))
            .clone()
    });
    (file, p)
}

/// Fetch, compile and instantiate chunk `file` over the main module's
/// exports (and the common chunk's, `common`).
async fn instance(file: String, common: Option<Promise>) -> Result<JsValue, JsValue> {
    let global = js_sys::global();
    let fetch: js_sys::Function = Reflect::get(&global, &"fetch".into())?.dyn_into()?;
    let url = format!("{}{file}", base());
    let r: web_sys::Response =
        JsFuture::from(fetch.call1(&global, &url.into())?.dyn_into::<Promise>()?).await?.dyn_into()?;
    if !r.ok() {
        return Err(format!("HTTP {}", r.status()).into());
    }
    let bytes = JsFuture::from(r.array_buffer()?).await?;
    let module: WebAssembly::Module = JsFuture::from(WebAssembly::compile(&bytes)).await?.dyn_into()?;
    let imports = Object::new();
    Reflect::set(&imports, &"m".into(), &wasm_bindgen::exports())?;
    if let Some(c) = common {
        let c: WebAssembly::Instance = JsFuture::from(c).await?.dyn_into()?;
        Reflect::set(&imports, &"c".into(), &c.exports())?;
    }
    JsFuture::from(WebAssembly::instantiate_module(&module, &imports)).await
}

//! The window's DevTools protocol over the app's private `ui` door, so
//! `ax surface`, `trigger`, `shot` and `eval` reach a window launched with
//! no flag (aio needed none) and no port is ever opened. A dev build's only,
//! as aio's control API was: a release build opens none. Each connection
//! speaks CDP's own messages, one JSON per line: `{id, method, params}` in,
//! `{id, result}` or `{id, error: {message}}` and events `{method, params}`
//! out. Electron's main process runs them through `webContents.debugger`
//! (main.js); this side gives every call a number of its own, so several
//! connections (a recording and a trigger) share the one window.
use aio::serde_json::{Value, json};
use std::collections::HashMap;

/// The door's name in the app's sockets dir (`aio::ipc::addr`).
pub const DOOR: &str = "ui";

/// Who asked what (pure): the calls in flight and the connections open.
#[derive(Default)]
pub(crate) struct Relay {
    next: u64,
    asks: HashMap<u64, (u64, Value)>,
    open: Vec<u64>,
}

impl Relay {
    /// A new connection's number.
    pub fn join(&mut self) -> u64 {
        self.next += 1;
        self.open.push(self.next);
        self.next
    }

    /// Connection `from`'s line: the control line for the runtime, or the
    /// answer to give at once (a refusal, or [`COMPONENTS`]).
    pub fn ask(&mut self, from: u64, line: &str) -> Result<String, String> {
        let m: Value =
            aio::serde_json::from_str(line).map_err(|e| answer(&Value::Null, Err(&format!("not JSON: {e}"))))?;
        let Some(method) = m["method"].as_str() else {
            return Err(answer(&m["id"], Err("no method")));
        };
        // aio's own: the view's component tree, answered here.
        if method == COMPONENTS {
            return Err(answer(&m["id"], Ok(&components())));
        }
        // `ax dev`'s template hot reload: the views built again, answered here.
        if method == HOT {
            return Err(answer(&m["id"], hot(&m["params"]).as_ref().map_err(String::as_str)));
        }
        // `ax dev`'s hot-patch: where this process is loaded, then a patch.
        if method == ASLR || method == PATCH {
            return Err(answer(&m["id"], patching(method, &m["params"]).as_ref().map_err(String::as_str)));
        }
        self.next += 1;
        self.asks.insert(self.next, (from, m["id"].clone()));
        Ok(json!({ "t": "cdp", "id": self.next, "method": method, "params": m.get("params").unwrap_or(&json!({})) })
            .to_string())
    }

    /// The runtime's `cdp` (an answer) or `cdp-event` line: to whom, what.
    /// An event goes to every connection; an answer to its asker, if open.
    pub fn heard(&mut self, m: &Value) -> Vec<(u64, String)> {
        if m["t"] == "cdp-event" {
            let e = json!({ "method": m["method"], "params": m["params"] }).to_string();
            return self.open.iter().map(|c| (*c, e.clone())).collect();
        }
        let Some((to, id)) = m["id"].as_u64().and_then(|n| self.asks.remove(&n)) else { return vec![] };
        let r = match m["error"].as_str() {
            Some(e) => Err(e),
            None => Ok(&m["result"]),
        };
        vec![(to, answer(&id, r))]
    }

    /// Connection `c` closed: its calls are forgotten; the last one out
    /// detaches the debugger (`Some`: the line that says so).
    pub fn leave(&mut self, c: u64) -> Option<String> {
        self.open.retain(|o| *o != c);
        self.asks.retain(|_, (from, _)| *from != c);
        self.open.is_empty().then(|| json!({ "t": "cdp-end" }).to_string())
    }
}

/// The door's own method: the running view's component tree,
/// `{"components": [{name, key, renders, updates, children}]}`, its last
/// renders, `"renders": [{component, key, at, ms, trigger}]` (a dev build's),
/// and `"total"` renders.
pub const COMPONENTS: &str = "Aio.components";

/// The door's own method: `ax dev`'s template hot reload. Its params:
/// `{"files": [file], "templates": [{file, line, col, idx, nodes (base64),
/// count}]}` (the files' templates edited since they were built, each by
/// where its `rsx!` call is written; the files' others as built); the
/// answer `{"windows": n}`, the windows told to build their views again.
pub const HOT: &str = "Aio.hot";

/// Apply a hot reload's templates (air's).
fn hot(params: &Value) -> Result<Value, String> {
    let (files, markup) = aio::ui::hot_markup(params)?;
    Ok(json!({ "windows": crate::views::hot(&files, markup) }))
}

/// The door's own methods for `ax dev`'s hot-patch (a dev build with
/// aio-shell's `dev`): `Aio.aslr` answers `{"aslr": n}`, where this process
/// is loaded (`aio_hotpatch::aslr_reference`, which a patch is linked
/// against), and on Windows `"base"`: a free span near this binary where a
/// patch DLL, linked at that fixed base, loads; `Aio.patch` takes `{"table": <an aio_hotpatch::JumpTable>}`, loads
/// the patch and answers `{"applied": true}`: the code calls the new
/// functions from then on (the views on air are built again on them).
pub const ASLR: &str = "Aio.aslr";
pub const PATCH: &str = "Aio.patch";

fn patching(method: &str, params: &Value) -> Result<Value, String> {
    #[cfg(feature = "dev")]
    {
        if method == ASLR {
            #[cfg(windows)]
            return Ok(json!({ "aslr": aio_hotpatch::aslr_reference(), "base": patch_base() }));
            #[cfg(not(windows))]
            return Ok(json!({ "aslr": aio_hotpatch::aslr_reference() }));
        }
        let table: aio_hotpatch::JumpTable =
            aio::serde_json::from_value(params["table"].clone()).map_err(|e| format!("not a jump table: {e}"))?;
        // The calls so far snapshotted as the old code left them: a later
        // start must not replay them through the patched code.
        let unsaved = aio::checkpoint_all();
        if !unsaved.is_empty() {
            return Err(format!("not patched, the state couldn't be saved first: {}", unsaved.join("; ")));
        }
        // SAFETY: a table `ax dev` linked against this very binary at the
        // address it answered (`Aio.aslr`).
        unsafe { aio_hotpatch::apply_patch(table) }.map_err(|e| format!("the patch didn't load: {e}"))?;
        #[cfg(all(windows, target_env = "msvc"))]
        refresh_symbols();
        Ok(json!({ "applied": true }))
    }
    #[cfg(not(feature = "dev"))]
    {
        _ = (method, params);
        Err("not built for hot-patching: ax dev builds with aio-shell's dev feature".into())
    }
}

/// Windows (MSVC): std's backtrace resolver (dbghelp) lists this process's
/// modules once, at the first backtrace; told again after each patch, a
/// backtrace in a patch DLL names its functions (from its PDB). Under std's
/// own dbghelp lock (backtrace-rs's `Local\RustBacktraceMutex<pid>`). No
/// dbghelp loaded: no backtrace yet, and the first one lists every module.
#[cfg(all(windows, target_env = "msvc", feature = "dev"))]
fn refresh_symbols() {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows_sys::Win32::System::Threading::{
        CreateMutexA, GetCurrentProcess, GetCurrentProcessId, INFINITE, ReleaseMutex, WaitForSingleObject,
    };
    let dll: Vec<u16> = "dbghelp.dll\0".encode_utf16().collect();
    // SAFETY: dbghelp's own entry point, called as std calls it, under its lock.
    unsafe {
        let module = GetModuleHandleW(dll.as_ptr());
        if module.is_null() {
            return;
        }
        let Some(f) = GetProcAddress(module, c"SymRefreshModuleList".as_ptr().cast()) else { return };
        let refresh: unsafe extern "system" fn(HANDLE) -> i32 = std::mem::transmute(f);
        let name = format!("Local\\RustBacktraceMutex{:08X}\0", GetCurrentProcessId());
        let lock = CreateMutexA(std::ptr::null(), 0, name.as_ptr());
        if lock.is_null() {
            return;
        }
        WaitForSingleObject(lock, INFINITE);
        refresh(GetCurrentProcess());
        ReleaseMutex(lock);
        CloseHandle(lock);
    }
}

/// Windows: the first 64 KiB-aligned free span of 256 MiB (ax's
/// `PATCH_ROOM`) past this binary's `main` within 1.5 GiB of it: a patch
/// reaches the binary's data rip-relative (±2 GiB). `None`: no such span.
#[cfg(all(windows, feature = "dev"))]
fn patch_base() -> Option<u64> {
    use windows_sys::Win32::System::Memory::{MEM_FREE, MEMORY_BASIC_INFORMATION, VirtualQuery};
    const ROOM: usize = 256 << 20;
    let main = aio_hotpatch::aslr_reference();
    let mut at = main;
    while at < main + (3 << 29) {
        // SAFETY: a query of this process's own address space into a local.
        let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<MEMORY_BASIC_INFORMATION>();
        if unsafe { VirtualQuery(at as *const _, &mut m, size) } == 0 {
            return None;
        }
        let end = m.BaseAddress as usize + m.RegionSize;
        let start = (at.max(m.BaseAddress as usize) + 0xffff) & !0xffff;
        if m.State == MEM_FREE && start + ROOM <= end {
            return Some(start as u64);
        }
        at = end;
    }
    None
}

/// What the dev build keeps; nothing before the window's view runs.
fn components() -> Value {
    crate::views::components()
}

/// A CDP answer line for call `id` (pure).
fn answer(id: &Value, r: Result<&Value, &str>) -> String {
    match r {
        Ok(v) => json!({ "id": id, "result": v }),
        Err(e) => json!({ "id": id, "error": { "message": e } }),
    }
    .to_string()
}

type Clients = HashMap<u64, tokio::sync::mpsc::UnboundedSender<String>>;

/// The relay and each connection's way out, shared by the door and the
/// control line.
#[derive(Clone, Default)]
pub(crate) struct Hub(std::sync::Arc<std::sync::Mutex<(Relay, Clients)>>);

/// This process's hub.
pub(crate) fn hub() -> &'static Hub {
    static HUB: std::sync::OnceLock<Hub> = std::sync::OnceLock::new();
    HUB.get_or_init(Hub::default)
}

impl Hub {
    /// A line the runtime said (`cdp`, `cdp-event`), passed on.
    pub fn heard(&self, m: &Value) {
        let mut g = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let (relay, clients) = &mut *g;
        for (to, line) in relay.heard(m) {
            if let Some(tx) = clients.get(&to) {
                _ = tx.send(line);
            }
        }
    }
}

/// Serve the `ui` door: each connection's calls go to the runtime over
/// `ctl`; without a window yet, they are refused.
pub(crate) async fn serve(mut l: aio::ipc::Listener, hub: Hub, ctl: crate::Ctl) {
    while let Ok(s) = l.accept().await {
        let (hub, ctl) = (hub.clone(), ctl.clone());
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
            let (r, mut w) = tokio::io::split(s);
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let me = {
                let mut g = hub.0.lock().unwrap_or_else(|p| p.into_inner());
                let me = g.0.join();
                g.1.insert(me, tx.clone());
                me
            };
            let out = tokio::spawn(async move {
                while let Some(l) = rx.recv().await {
                    if w.write_all(format!("{l}\n").as_bytes()).await.is_err() {
                        break;
                    }
                }
            });
            let mut lines = tokio::io::BufReader::new(r).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let asked = hub.0.lock().unwrap_or_else(|p| p.into_inner()).0.ask(me, &l);
                match asked {
                    Ok(line) => {
                        let sent = match ctl.lock().await.as_mut() {
                            Some(c) => c.write_all(format!("{line}\n").as_bytes()).await.is_ok(),
                            None => false,
                        };
                        if !sent {
                            let id: Value = aio::serde_json::from_str(&line).unwrap_or_default();
                            hub.heard(&json!({ "t": "cdp", "id": id["id"], "error": "the window is not open yet" }));
                        }
                    }
                    Err(refusal) => _ = tx.send(refusal),
                }
            }
            let end = {
                let mut g = hub.0.lock().unwrap_or_else(|p| p.into_inner());
                g.1.remove(&me);
                g.0.leave(me)
            };
            if let (Some(end), Some(c)) = (end, ctl.lock().await.as_mut()) {
                _ = c.write_all(format!("{end}\n").as_bytes()).await;
            }
            drop(tx);
            _ = out.await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_call_gets_its_own_number_and_its_answer_finds_the_asker() {
        let mut r = Relay::default();
        let (a, b) = (r.join(), r.join());
        let to_rt = |l: String| aio::serde_json::from_str::<Value>(&l).unwrap();
        let ka = to_rt(r.ask(a, r#"{"id":1,"method":"Runtime.evaluate","params":{"expression":"1"}}"#).unwrap());
        let kb = to_rt(r.ask(b, r#"{"id":1,"method":"Page.captureScreenshot"}"#).unwrap());
        assert_eq!((ka["t"].as_str(), ka["method"].as_str()), (Some("cdp"), Some("Runtime.evaluate")));
        assert_eq!(ka["params"]["expression"], "1");
        assert_eq!(kb["params"], json!({}), "no params: an empty object");
        assert_ne!(ka["id"], kb["id"], "both asked as 1, told apart");
        let got = r.heard(&json!({ "t": "cdp", "id": kb["id"], "error": "No target" }));
        assert_eq!(got, [(b, r#"{"error":{"message":"No target"},"id":1}"#.to_string())]);
        let got = r.heard(&json!({ "t": "cdp", "id": ka["id"], "result": { "v": 2 } }));
        assert_eq!(got, [(a, r#"{"id":1,"result":{"v":2}}"#.to_string())]);
        assert!(r.heard(&json!({ "t": "cdp", "id": ka["id"], "result": {} })).is_empty(), "answered once");
        let ev = r.heard(&json!({ "t": "cdp-event", "method": "Page.screencastFrame", "params": { "n": 1 } }));
        assert_eq!(ev.iter().map(|(c, _)| *c).collect::<Vec<_>>(), [a, b], "an event reaches every connection");
        assert_eq!(ev[0].1, r#"{"method":"Page.screencastFrame","params":{"n":1}}"#);
    }

    #[test]
    fn a_bad_line_is_refused_and_the_last_one_out_detaches() {
        let mut r = Relay::default();
        let (a, b) = (r.join(), r.join());
        assert!(r.ask(a, "nope").unwrap_err().contains("not JSON"));
        // aio's own method never reaches the runtime: the tree, answered here.
        let tree = r.ask(a, r#"{"id":3,"method":"Aio.components"}"#).unwrap_err();
        assert!(tree.starts_with(r#"{"id":3,"result":{"components":["#), "{tree}");
        assert_eq!(r.ask(a, r#"{"id":7}"#).unwrap_err(), r#"{"error":{"message":"no method"},"id":7}"#);
        let k: Value = aio::serde_json::from_str(&r.ask(a, r#"{"id":1,"method":"X"}"#).unwrap()).unwrap();
        assert_eq!(r.leave(a), None, "b still holds it");
        assert!(r.heard(&json!({ "t": "cdp", "id": k["id"], "result": {} })).is_empty(), "a's answer has no one");
        assert!(r.heard(&json!({ "t": "cdp-event", "method": "E" })).iter().all(|(c, _)| *c == b));
        assert_eq!(r.leave(b).as_deref(), Some(r#"{"t":"cdp-end"}"#));
        // Electron's side: run by its own debugger, let go at the end.
        let js = include_str!("main.js");
        assert!(js.contains("if (m.t === 'cdp') devtools(m)") && js.contains("if (m.t === 'cdp-end') undevtools()"));
        assert!(
            js.contains("dbg.sendCommand(m.method, m.params)")
                && js.contains("tell({ t: 'cdp-event', method, params })")
        );
    }

    #[tokio::test]
    async fn the_door_relays_calls_to_the_runtime_and_its_answers_back() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let d = tempfile::tempdir().unwrap();
        let addr = aio::ipc::addr(d.path(), DOOR);
        let (ours, electron) = tokio::io::duplex(1 << 16);
        let (er, mut ew) = tokio::io::split(electron);
        let ctl: crate::Ctl = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let hub = Hub::default();
        tokio::spawn(serve(aio::ipc::Listener::bind(&addr).unwrap(), hub.clone(), ctl.clone()));
        let (cr, mut cw) = tokio::io::split(aio::ipc::connect(&addr).await.unwrap());
        let mut client = tokio::io::BufReader::new(cr).lines();
        // No window yet: refused at once, never left waiting.
        cw.write_all(b"{\"id\":1,\"method\":\"Runtime.evaluate\"}\n").await.unwrap();
        let no: Value = aio::serde_json::from_str(&client.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!((no["id"].as_u64(), no["error"]["message"].as_str()), (Some(1), Some("the window is not open yet")));
        // The window's control line is up: Electron answers what it is asked.
        let (r, w) = tokio::io::split(Box::new(ours) as aio::ipc::Stream);
        *ctl.lock().await = Some(w);
        let hub2 = hub.clone();
        tokio::spawn(async move {
            let mut l = tokio::io::BufReader::new(r).lines();
            while let Ok(Some(l)) = l.next_line().await {
                hub2.heard(&aio::serde_json::from_str(&l).unwrap());
            }
        });
        cw.write_all(b"{\"id\":2,\"method\":\"Runtime.evaluate\",\"params\":{\"expression\":\"1+1\"}}\n")
            .await
            .unwrap();
        let mut runtime = tokio::io::BufReader::new(er).lines();
        let asked: Value = aio::serde_json::from_str(&runtime.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!((asked["t"].as_str(), asked["params"]["expression"].as_str()), (Some("cdp"), Some("1+1")));
        let ev = json!({ "t": "cdp-event", "method": "Page.loadEventFired", "params": {} });
        let back = json!({ "t": "cdp", "id": asked["id"], "result": { "result": { "value": 2 } } });
        ew.write_all(format!("{ev}\n{back}\n").as_bytes()).await.unwrap();
        let e: Value = aio::serde_json::from_str(&client.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(e["method"], "Page.loadEventFired");
        let a: Value = aio::serde_json::from_str(&client.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!((a["id"].as_u64(), a["result"]["result"]["value"].as_u64()), (Some(2), Some(2)));
        // Gone: the debugger is let go.
        drop((cw, client));
        assert_eq!(runtime.next_line().await.unwrap().as_deref(), Some(r#"{"t":"cdp-end"}"#));
    }
}

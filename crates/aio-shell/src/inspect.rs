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
/// `{"components": [{name, renders, ms, children}]}`, its last renders,
/// `"renders": [{component, at, ms, trigger}]`, and `"total"` renders.
pub const COMPONENTS: &str = "Aio.components";

/// What the dev build keeps; nothing before the window's view runs.
fn components() -> Value {
    #[cfg(debug_assertions)]
    if let Some(t) = crate::renders::TREE.get() {
        let t = t.lock().unwrap_or_else(|e| e.into_inner());
        let (renders, total) = t.renders();
        return json!({ "components": t.json(), "renders": renders, "total": total });
    }
    json!({ "components": [], "renders": [], "total": 0 })
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

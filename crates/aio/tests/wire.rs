//! A UI driven over the real wire (aio's `testUI`'s wire emulation): its calls
//! cross the sync protocol to a served cell, the server decides who may make
//! them, and a slow link is a slow link.
use aio::act;
use aio::prelude::*;
use aio::testing::{Mount, TestServer, TestUi};
use aio::ui::prelude::*;
use std::time::Duration;

#[cell(shared)]
pub struct Shop {
    pub stock: u32,
    pub owner: String,
}

#[methods]
impl Shop {
    #[client]
    pub fn sell(&mut self) {
        self.stock += 1;
    }

    #[client(role = "admin")]
    pub fn restock(&mut self) {
        self.stock += 100;
    }

    /// Not `#[client]`: the server's own.
    pub fn give(&mut self, to: String) {
        self.owner = to;
    }
}

aio::client!(Shop, shop);

fn till() -> View {
    use_live();
    let s = shop();
    let waiting = use_pending(None);
    rsx! {
        p { "data-t": "stock", "{s.stock}" }
        p { "data-t": "owner", "{s.owner}" }
        p { "data-t": "waiting", "{waiting}" }
        button { "data-t": "sell", onclick: |_| _ = act!(sell()), "Sell" }
        button { "data-t": "restock", onclick: |_| _ = act!(restock()), "Restock" }
        button { "data-t": "take", onclick: |_| _ = act!(give("me".into())), "Take" }
    }
}

/// A UI on a served `Shop` (a background runtime serves it), `latency` each way.
fn wired(latency: Duration) -> (TestUi, TestServer<Shop>, tokio::runtime::Runtime) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(TestServer::start(Handle::<Shop>::memory(), Default::default())).unwrap();
    let source = rt.block_on(server.ui_source("ana", latency)).unwrap();
    let ui = Mount::default().before(move || *HANDLE.write() = source).app(till);
    (ui, server, rt)
}

/// The server's refusals, once one came (they come after the reply).
fn refused(ui: &mut TestUi) -> Vec<String> {
    for _ in 0..500 {
        let said = ui.failures();
        if !said.is_empty() {
            return said;
        }
        ui.wait(Duration::from_millis(10));
    }
    panic!("the server refused nothing");
}

#[test]
fn a_click_crosses_the_wire_and_the_server_decides_who_may() {
    let (mut ui, server, _rt) = wired(Duration::ZERO);
    ui.click("sell");
    ui.until("the server sold it", || server.state().stock == 1);
    // Not a #[client] method, and a role ana lacks: shown at once, then refused and rolled back.
    for (t, why) in
        [("take", "give: access denied: `give` is not a #[client] method"), ("restock", "needs the role admin")]
    {
        ui.click(t);
        let said = refused(&mut ui);
        assert!(said.len() == 1 && said[0].contains(why), "{said:?}");
        ui.until("rolled back", || HANDLE.read_untracked().pending(None) == 0);
        assert_eq!((ui.text("stock"), ui.text("owner")), ("1".into(), String::new()), "{t}: rolled back");
    }
    assert_eq!((server.state().stock, server.state().owner.as_str()), (1, ""));
}

#[test]
fn a_slow_link_shows_the_call_at_once_and_waits_for_the_server() {
    let (mut ui, server, _rt) = wired(Duration::from_millis(300));
    ui.click("sell");
    assert_eq!((ui.text("stock"), ui.text("waiting")), ("1".into(), "1".into()), "predicted, waiting");
    assert_eq!(server.state().stock, 0, "not there yet");
    ui.until("confirmed", || HANDLE.read_untracked().pending(None) == 0);
    ui.until_text("waiting", "0");
    assert_eq!(server.state().stock, 1);
}

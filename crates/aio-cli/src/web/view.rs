// The board's view: painted on the server, then drawn in the page.
use dioxus::prelude::*;

aio::client!(Board, board);

/// A box drawn by the page's own JS: its props in, an event back.
const DOUBLE: aio::kit::Island =
    aio::kit::Island::js("return (el, p, emit) => { el.textContent = 'island ' + p.n; emit(p.n * 2); };");

fn view() -> Element {
    use_live();
    let b = board();
    aio::head::use_head(
        aio::head::Head::title(format!("{} items", b.items.len())).meta(&[("name", "description"), ("content", "board")]),
    );
    let link = if use_status()().online { "online" } else { "offline" };
    let mut doubled = use_signal(|| 0i64);
    rsx! {
        ul { id: "items",
            for i in b.items.iter() {
                li { "{i}" }
            }
        }
        p { id: "link", "{link}" }
        button { id: "add", onclick: move |_| _ = aio::act!(add("ui".to_string())), "+" }
        input { id: "auto", autofocus: true }
        aio::kit::WebIsland {
            id: "island",
            island: DOUBLE,
            props: aio::serde_json::json!({ "n": b.items.len() }),
            onevent: move |v: i64| doubled.set(v),
        }
        p { id: "doubled", "{doubled}" }
    }
}

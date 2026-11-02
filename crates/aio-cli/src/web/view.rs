// The board's view, drawn in the page.
use aio::ui::prelude::*;

aio::client!(Board, board);

/// A box drawn by the page's own JS: its props in, an event back.
const DOUBLE: aio::kit::Island =
    aio::kit::Island::js("return (el, p, emit) => { el.textContent = 'island ' + p.n; emit(p.n * 2); };");

fn view() -> View {
    use_live();
    let items = part(|b| &b.items);
    aio::head::use_head(move || {
        aio::head::Head::title(format!("{} items", items.with(Vec::len))).meta(&[("name", "description"), ("content", "board")])
    });
    let status = use_status();
    let link = memo(move || if status.get().online { "online" } else { "offline" });
    let doubled = signal(|| 0i64);
    rsx! {
        ul { id: "items",
            for (k, i) in items.get().into_iter().enumerate() {
                li { key: "{k}", "{i}" }
            }
        }
        p { id: "link", "{link}" }
        button { id: "add", onclick: move |_| _ = aio::act!(add("ui".to_string())), "+" }
        input { id: "auto", autofocus: true }
        aio::kit::WebIsland {
            id: "island",
            island: DOUBLE,
            props: aio::serde_json::json!({ "n": items.with(Vec::len) }),
            onevent: move |v: i64| doubled.set(v),
        }
        p { id: "doubled", "{doubled}" }
    }
}

// The page of the web build's test on air: the page's own cell, its view
// drawn by air in the browser, driven from the
// test through its DOM.
use aio::ui::prelude::*;
use wasm_bindgen::prelude::*;

#[aio::cell(persist, id = "tally")]
pub struct Tally {
    pub n: i64,
}

#[aio::methods]
impl Tally {
    pub fn add(&mut self, by: i64) {
        self.n += by;
    }
}

aio::client!(Tally, tally);

#[component]
fn Row(label: String) -> View {
    let hits = signal(|| 0u32);
    rsx! { li { "data-t": "{label}", onclick: move |_| hits += 1, "{label} {hits}" } }
}

/// A panic in the page's Rust, for the test to see where it is said.
fn boom() {
    panic!("boom in the page");
}

#[component]
fn App() -> View {
    use_live();
    let n = part(|c| &c.n);
    let typed = signal(String::new);
    let rows = signal(|| vec!["a".to_string(), "b".to_string()]);
    rsx! {
        main {
            h1 { "data-t": "n", "{n}" }
            button { "data-t": "add", onclick: move |_| _ = act!(add(1)), "Add" }
            input { "data-t": "name", value: typed, oninput: move |e: FormEvent| typed.set(e.value()) }
            p { "data-t": "typed", "{typed}" }
            ul {
                for r in rows.get() {
                    Row { key: "{r}", label: r }
                }
            }
            button { "data-t": "flip", onclick: move |_| rows.update(|r| r.reverse()), "Flip" }
            button { "data-t": "boom", onclick: |_| boom(), "Boom" }
        }
    }
}

#[wasm_bindgen(start)]
fn start() {
    _ = SOURCE.set(aio::Source::Local(aio::Handle::saved("air-web-test")));
    aio::web::launch(App);
}

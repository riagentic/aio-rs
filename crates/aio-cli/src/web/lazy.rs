// The page of the web build's split test: a lazy view, its code in a
// chunk the page fetches when it first shows, and work off the page's
// thread in a worker running the page's own module.
use aio::kit::{Defer, Markdown, Trigger};
use aio::ui::prelude::*;
use wasm_bindgen::prelude::*;

/// Whether this runs off the page: a worker has no `document`.
fn off_page() -> bool {
    !js_sys::Reflect::has(&js_sys::global(), &"document".into()).unwrap_or(true)
}

/// The primes below `n`, and whether they were counted off the page.
fn primes(n: usize) -> (usize, bool) {
    let mut sieve = vec![true; n];
    let mut found = 0;
    for i in 2..n {
        if sieve[i] {
            found += 1;
            (i.saturating_mul(i)..n).step_by(i).for_each(|j| sieve[j] = false);
        }
    }
    (found, off_page())
}

/// Never ends: stopped from the page.
fn spin(_: ()) -> u32 {
    loop {
        std::hint::spin_loop();
    }
}

fn boom(_: ()) -> u32 {
    panic!("boom in the worker");
}

/// Only the lazy view runs it: its code is in the view's chunk, which the
/// worker loads before running it.
fn heavy_sum(n: u64) -> (u64, bool) {
    ((1..=n).map(|i| i * i).sum(), off_page())
}

/// Shows a job's end in `to`.
fn shown<T: std::fmt::Debug + 'static>(to: Signal<String>, job: impl std::future::Future<Output = aio::Result<T>> + 'static) {
    spawn(async move {
        to.set(match job.await {
            Ok(v) => format!("{v:?}"),
            Err(e) => format!("refused: {e}"),
        })
    });
}

/// The lazy view: markdown (the kit's parser, used nowhere else) and its
/// own state, clicks and work.
#[component]
fn Heavy() -> View {
    let hits = signal(|| 0u32);
    let sum = signal(String::new);
    rsx! {
        section { "data-t": "heavy",
            Markdown { source: "# Heavy\n\nOnly *this* view parses markdown." }
            button { "data-t": "hit", onclick: move |_| hits += 1, "hits {hits}" }
            button { "data-t": "sum", onclick: move |_| shown(sum, aio::effect::off_thread("sum", heavy_sum, 10)), "Sum" }
            p { "data-t": "summed", "{sum}" }
        }
    }
}

#[component]
fn App() -> View {
    let open = signal(|| false);
    let counted = signal(String::new);
    let spun = signal(String::new);
    let ticks = signal(|| 0u32);
    rsx! {
        main {
            button { "data-t": "open", onclick: move |_| open.set(true), "Open" }
            button { "data-t": "close", onclick: move |_| open.set(false), "Close" }
            if open.get() {
                Defer {
                    trigger: Trigger::Now,
                    load: aio::lazy!(rsx! { Heavy {} }),
                    loading: rsx! { p { "data-t": "loading", "Loading" } },
                    error: move |e: String| rsx! { p { "data-t": "failed", "{e}" } },
                }
            }
            button { "data-t": "count", onclick: move |_| shown(counted, aio::effect::off_thread("primes", primes, 5_000_000)), "Count" }
            p { "data-t": "counted", "{counted}" }
            button { "data-t": "spin", onclick: move |_| shown(spun, aio::effect::off_thread("spin", spin, ())), "Spin" }
            button { "data-t": "stop", onclick: move |_| _ = aio::effect::cancel_blocking("spin"), "Stop" }
            button { "data-t": "boom", onclick: move |_| shown(spun, aio::effect::off_thread("boom", boom, ())), "Boom" }
            p { "data-t": "spun", "{spun}" }
            button { "data-t": "tick", onclick: move |_| ticks += 1, "ticks {ticks}" }
        }
    }
}

#[wasm_bindgen(start)]
fn start() {
    aio::web::launch(App);
}

//! How long `air::ssr::render_to_string` takes for a table of 2, 50 and
//! 1000 rows (the median of many renders, after a warm-up), its rows from
//! a signal and from a plain list:
//!
//! ```text
//! cargo run --release -p aio-rs-air --example ssr_speed
//! ```
//!
//! aio's `renderToString` on the same table, for comparison
//! (`docs/ui/views.md`, "Server rendering and hydration"):
//!
//! ```text
//! import { jsx } from "<aio>/src/jsx-runtime.ts";
//! import { renderToString } from "<aio>/src/air.ts";
//! const row = (r) => jsx("tr", { children: [
//!   jsx("td", { class: "id", children: String(r.id) }),
//!   jsx("td", { children: jsx("a", { children: r.label }) }),
//!   jsx("td", { children: jsx("button", { onClick: () => {}, children: "x" }) }),
//! ] }, r.id);
//! const app = (rows) => jsx("table", { children: jsx("tbody", { children: rows.map(row) }) });
//! for (const n of [2, 50, 1000]) {
//!   const rows = Array.from({ length: n }, (_, i) => ({ id: i, label: `row ${i}` }));
//!   const runs = Math.max(200, 200000 / n), t = [];
//!   for (let i = 0; i < runs / 4; i++) renderToString(app(rows));
//!   for (let i = 0; i < runs; i++) { const a = performance.now(); renderToString(app(rows)); t.push(performance.now() - a); }
//!   t.sort((a, b) => a - b);
//!   console.log(n, (t[t.length >> 1] * 1000).toFixed(1), "µs");
//! }
//! ```
use air::{View, rsx, signal};
use std::rc::Rc;
use std::time::Instant;

#[derive(Clone, PartialEq)]
struct Row {
    id: u32,
    label: String,
}

fn table(rows: Vec<Row>) -> View {
    rsx! {
        table { tbody {
            for r in rows {
                tr { key: "{r.id}",
                    td { class: "id", "{r.id}" }
                    td { a { "{r.label}" } }
                    td { button { onclick: move |_| {}, "x" } }
                }
            }
        } }
    }
}

/// The median of `runs` renders of `app`, in µs.
fn median(app: impl Fn() -> View + Clone + 'static, runs: usize) -> f64 {
    for _ in 0..runs / 4 {
        air::ssr::render_to_string(app.clone()).unwrap();
    }
    let mut t: Vec<f64> = (0..runs)
        .map(|_| {
            let a = Instant::now();
            std::hint::black_box(air::ssr::render_to_string(app.clone()).unwrap());
            a.elapsed().as_secs_f64() * 1e6
        })
        .collect();
    t.sort_by(f64::total_cmp);
    t[t.len() / 2]
}

fn main() {
    for n in [2u32, 50, 1000] {
        let rows: Rc<Vec<Row>> = Rc::new((0..n).map(|id| Row { id, label: format!("row {id}") }).collect());
        let runs = (200_000 / n as usize).max(200);
        let plain = {
            let rows = rows.clone();
            move || table(rows.to_vec())
        };
        let followed = move || {
            let rows = signal({
                let rows = rows.clone();
                move || rows.to_vec()
            });
            table(rows.get())
        };
        println!(
            "{n} rows: {:.1} µs (a plain list), {:.1} µs (from a signal)",
            median(plain, runs),
            median(followed, runs)
        );
    }
}

//! Chart, Heatmap and Canvas (feature `draw`) in the real page.
use aio::kit::*;
use aio::testing::TestUi;
use aio::ui::prelude::*;
use std::time::Duration;

/// The canvas's pixel at `(x, y)` (device pixels) as `[r, g, b, a]`.
fn pixel(ui: &mut TestUi, t: &str, x: u32, y: u32) -> Vec<u64> {
    let v = ui.eval(&format!(
        "Array.from(document.querySelector('[data-t=\"{t}\"] canvas').getContext('2d').getImageData({x}, {y}, 1, 1).data)"
    ));
    v.as_array().map(|a| a.iter().filter_map(|n| n.as_u64()).collect()).unwrap_or_default()
}

fn canvas() -> View {
    let wide = signal(|| false);
    let said = signal(String::new);
    let size = signal(String::new);
    let draw = memo(move || {
        let mut d = Draw::new();
        d.fill("#ff0000").rect(0.0, 0.0, 10.0, 10.0);
        d.fill("var(--probe)").rect(10.0, 0.0, 10.0, 10.0);
        let px = Pixels::indexed(2, 1, &[0, 1], &[[0, 0, 255, 255], [0, 255, 0, 255]]);
        d.smooth(false).pixels(&px, 20.0, 0.0, 20.0, 10.0);
        d.save().translate(0.0, 10.0).stroke("#0000ff", 4.0).line(0.0, 5.0, 40.0, 5.0).restore();
        if wide.get() {
            d.fill("#000000").rect(100.0, 0.0, 10.0, 10.0);
        }
        d
    });
    rsx! {
        div { style: "--probe: #00ff00; width: 200px",
            Canvas { "data-t": "cv", draw: draw,
                onpointer: move |p: CanvasPointer| said.set(format!("{:?} {:.0} {:.0} {}", p.kind, p.x, p.y, p.buttons)),
                onsize: move |(w, h): (f64, f64)| size.set(format!("{w}x{h}")),
                style: if wide.get() { "height: 80px" } else { "height: 40px" },
            }
        }
        button { "data-t": "wide", onclick: move |_| wide.toggle(), "wide" }
        p { "data-t": "said", "{said}" }
        p { "data-t": "size", "{size}" }
    }
}

#[test]
fn a_canvas_paints_its_draw_list_and_paints_again_when_it_changes() {
    let mut ui = TestUi::new(canvas);
    ui.until_text("size", "200x40");
    ui.until("the first paint", || true);
    assert_eq!(pixel(&mut ui, "cv", 5, 5), [255, 0, 0, 255], "a filled rect");
    assert_eq!(pixel(&mut ui, "cv", 15, 5), [0, 255, 0, 255], "a token read from the page");
    assert_eq!(pixel(&mut ui, "cv", 25, 5), [0, 0, 255, 255], "indexed pixels through their palette");
    assert_eq!(pixel(&mut ui, "cv", 35, 5), [0, 255, 0, 255], "stretched as blocks");
    assert_eq!(pixel(&mut ui, "cv", 20, 15), [0, 0, 255, 255], "a stroked line, translated");
    assert_eq!(pixel(&mut ui, "cv", 105, 5), [0, 0, 0, 0], "nothing there yet");
    ui.click("wide");
    ui.until_text("size", "200x80");
    assert_eq!(ui.eval("document.querySelector('[data-t=cv] canvas').height"), 80, "the bitmap follows the box");
    let r = ui.eval("(() => { const a = document.querySelector('[data-t=cv]').getBoundingClientRect(), b = document.querySelector('[data-t=cv] canvas').getBoundingClientRect(); return [a.x === b.x, a.y === b.y, a.width === b.width, a.height === b.height]; })()");
    assert_eq!(r, serde_json::json!([true, true, true, true]), "the canvas covers its box, its style changed or not");
    assert_eq!(pixel(&mut ui, "cv", 105, 5), [0, 0, 0, 255], "a new list repaints");
    assert_eq!(pixel(&mut ui, "cv", 5, 5), [255, 0, 0, 255]);
}

#[test]
fn a_canvas_sends_pointer_events_in_its_own_coordinates() {
    let mut ui = TestUi::new(canvas);
    ui.until_text("size", "200x40");
    ui.click_at("cv", 0.25, 0.5);
    ui.until_text("said", "Click 50 20 0");
}

fn view_box() -> View {
    let said = signal(String::new);
    let mut d = Draw::new();
    d.fill("#123456").rect(1.0, 0.0, 1.0, 1.0);
    rsx! {
        Canvas { "data-t": "vb", draw: d, view: Some((4.0, 2.0)), style: "width: 200px; height: 100px",
            onpointer: move |p: CanvasPointer| if p.kind == PointerKind::Down { said.set(format!("{:.2} {:.2}", p.x, p.y)) },
        }
        p { "data-t": "said", "{said}" }
    }
}

#[test]
fn a_canvas_view_stretches_the_drawing_and_maps_the_pointer_back() {
    let mut ui = TestUi::new(view_box);
    ui.until("painted", || true);
    ui.until_within(Duration::from_secs(5), "the cell painted", || true);
    assert_eq!(pixel(&mut ui, "vb", 75, 10), [0x12, 0x34, 0x56, 255], "cell (0, 1) is 50 px wide");
    assert_eq!(pixel(&mut ui, "vb", 25, 10), [0, 0, 0, 0]);
    ui.click_at("vb", 0.375, 0.25);
    ui.until_text("said", "1.50 0.50");
}

const HEAT: (usize, usize) = (256, 256);

fn heat_data(shift: f32) -> Vec<f32> {
    (0..HEAT.0 * HEAT.1).map(|i| ((i % HEAT.1) as f32 + shift) % HEAT.1 as f32).collect()
}

fn heatmap() -> View {
    let shift = signal(|| 0.0f32);
    let sel = signal(|| None::<(usize, usize)>);
    rsx! {
        div { style: "width: 512px",
            Heatmap { "data-t": "heat", rows: HEAT.0, cols: HEAT.1, data: heat_data(shift.get()), sel: sel.get(),
                range: Some((0.0, 255.0)),
                onpick: move |rc: (usize, usize)| sel.set(Some(rc)),
                row_labels: (0..HEAT.0).map(|r| format!("r{r}")).collect::<Vec<_>>(),
            }
        }
        button { "data-t": "shift", onclick: move |_| shift.set(shift.get() + 128.0), "shift" }
        p { "data-t": "sel", {format!("{:?}", sel.get())} }
    }
}

/// The heat map's paint: its canvas's last paint time (ms).
fn paint_ms(ui: &mut TestUi, t: &str) -> f64 {
    ui.eval(&format!("document.querySelector('[data-t=\"{t}\"] canvas').aioPaintMs")).as_f64().unwrap_or(f64::NAN)
}

#[test]
fn a_heatmap_is_one_bitmap_of_its_cells_in_its_palette() {
    let mut ui = TestUi::new(heatmap);
    ui.until("painted", || true);
    let w = ui.eval("document.querySelector('[data-t=heat] canvas').width").as_u64().unwrap();
    assert_eq!(w, ui.rect("heatmapCells").width.round() as u64, "the cells fill their box");
    // Column 0 is the lowest value, the last the highest.
    let low = pixel(&mut ui, "heat", 0, 0);
    let high = pixel(&mut ui, "heat", w as u32 - 1, 0);
    assert_eq!(low, [0x44, 0x01, 0x54, 255], "viridis's low end");
    assert_eq!(high, [0xfd, 0xe7, 0x25, 255], "viridis's high end");
    assert_eq!(ui.count("[data-t=heat] canvas"), 1, "65 536 cells, one canvas");
    eprintln!("[bench] 256x256 heatmap first paint: {:.2} ms", paint_ms(&mut ui, "heat"));
    let mut times = vec![];
    for _ in 0..10 {
        let t0 = std::time::Instant::now();
        ui.click("shift");
        ui.until("repainted", || true);
        times.push((t0.elapsed().as_secs_f64() * 1e3, paint_ms(&mut ui, "heat")));
    }
    assert_eq!(pixel(&mut ui, "heat", 0, 0), low, "an even number of shifts: back where it was");
    times.sort_by(|a, b| a.1.total_cmp(&b.1));
    eprintln!(
        "[bench] 256x256 heatmap update, page paint (ms) sorted: {:?}",
        times.iter().map(|t| (t.1 * 100.0).round() / 100.0).collect::<Vec<_>>()
    );
    let build = std::time::Instant::now();
    for i in 0..20 {
        let d = heat_data(i as f32);
        let mut draw = Draw::new();
        let px = Pixels::indexed(256, 256, &d.iter().map(|v| (*v as u8).min(254)).collect::<Vec<_>>(), &[[0; 4]; 255]);
        draw.pixels(&px, 0.0, 0.0, 1.0, 1.0);
        std::hint::black_box(draw);
    }
    eprintln!("[bench] 256x256 pixels encoded in the app: {:.3} ms each", build.elapsed().as_secs_f64() * 1e3 / 20.0);
}

fn small_heat() -> View {
    let sel = signal(|| None::<(usize, usize)>);
    rsx! {
        div { style: "width: 440px",
            Heatmap { "data-t": "heat", rows: 4, cols: 4, data: (0..16).map(|i| i as f32).collect::<Vec<_>>(), sel: sel.get(),
                height: 200.0, palette: Palette::Magma,
                onpick: move |rc: (usize, usize)| sel.set(Some(rc)),
                col_labels: vec!["a".to_string(), "b".into(), "c".into(), "d".into()],
            }
        }
        p { "data-t": "sel", {format!("{:?}", sel.get())} }
    }
}

#[test]
fn a_heatmap_picks_the_cell_clicked_or_chosen_with_keys_and_tells_the_hovered_value() {
    let mut ui = TestUi::new(small_heat);
    ui.click_at("heatmapCells", 0.6, 0.1);
    ui.until_text("sel", "Some((0, 2))");
    assert_eq!(ui.count("[data-t=heat] .aio-heatmap__sel"), 1, "the selection is outlined");
    ui.hover("heatmapCells");
    ui.until_text("heatmapTip", "2, c: 10");
    ui.focus("heatmapCells");
    ui.press("heatmapCells", "ArrowRight");
    ui.press("heatmapCells", "ArrowDown");
    ui.press("heatmapCells", "Enter");
    ui.until_text("sel", "Some((3, 3))");
    assert_eq!(ui.attr("heatmapCells", "aria-label").as_deref(), Some("Heat map, 4 rows by 4 columns, values 0 to 15"));
}

fn chart() -> View {
    let many = signal(|| false);
    let series = memo(move || {
        let n = if many.get() { 10_000 } else { 5 };
        let mut s = vec![Series::line("load", (0..n).map(|i| (i as f64, (i % 7) as f64)).collect::<Vec<_>>())];
        if !many.get() {
            s.push(Series::scatter("peaks", vec![(1.0, 6.0), (3.0, 2.0)]));
        }
        s
    });
    rsx! {
        div { style: "width: 600px",
            Chart { "data-t": "chart", series: series.get(), title: "Load", x: Axis::linear("time"), y: Axis::linear("jobs") }
            Chart { "data-t": "roof", series: vec![Series::scatter("kernels", vec![(0.5, 20.0), (8.0, 900.0)])],
                x: Axis::log("FLOP/byte"), y: Axis::log("GFLOP/s") }
            Chart { "data-t": "dots", series: vec![Series::scatter("p", vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)])] }
            Chart { "data-t": "bars", series: vec![Series::bar("a", vec![(0.0, 3.0), (1.0, 5.0), (2.0, 1.0)]), Series::bar("b", vec![(0.0, 2.0), (1.0, 4.0), (2.0, 6.0)])],
                x: Axis::names("kernel", ["gemm", "conv", "fft"]) }
        }
        button { "data-t": "many", onclick: move |_| many.toggle(), "many" }
    }
}

/// The texts of `sel` inside handle `t`.
fn texts(ui: &mut TestUi, t: &str, sel: &str) -> Vec<String> {
    let v = ui.eval(&format!("[...document.querySelectorAll('[data-t=\"{t}\"] {sel}')].map((e) => e.textContent)"));
    serde_json::from_value(v).unwrap_or_default()
}

/// Wait until the texts of `sel` inside `t` are `want` (a chart lays out
/// again once its box's width is known).
#[track_caller]
fn until_texts(ui: &mut TestUi, t: &str, sel: &str, want: &[&str]) {
    let start = std::time::Instant::now();
    loop {
        let got = texts(ui, t, sel);
        if got == want {
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "{t} {sel}: {got:?}, not {want:?}");
        ui.wait(Duration::from_millis(20));
    }
}

#[test]
fn a_chart_draws_its_series_on_round_ticks_with_a_legend_and_says_what_it_shows() {
    let mut ui = TestUi::new(chart);
    ui.until("drawn", || true);
    until_texts(&mut ui, "chart", "svg > g > g > text", &["0", "2", "4", "6", "0", "1", "2", "3", "4"]);
    assert_eq!(texts(&mut ui, "chart", ".aio-chart__legend"), ["loadpeaks"]);
    assert_eq!(ui.count("[data-t=chart] path"), 2);
    assert!(ui.attr("chart", "class").is_some_and(|c| c == "aio-chart"));
    assert_eq!(texts(&mut ui, "chart", "svg title"), ["Load"]);
    let desc = texts(&mut ui, "chart", "svg desc").join("");
    assert_eq!(
        desc,
        "load (line): 5 points, time 0 to 4, jobs 0 to 4. peaks (dots): 2 points, time 1 to 3, jobs 2 to 6."
    );
    until_texts(&mut ui, "roof", "svg > g > g > text", &["10", "100", "1k", "0.1", "1", "10"]);
    assert_eq!(texts(&mut ui, "bars", "svg > g > g > text")[4..], ["gemm", "conv", "fft"], "categories by name");
    assert_eq!(ui.count("[data-t=bars] .aio-chart__legend > span"), 2);
}

#[test]
fn a_chart_tells_the_nearest_point_on_hover_and_thins_a_long_line_to_its_pixels() {
    let mut ui = TestUi::new(chart);
    ui.hover("roof");
    assert_eq!(ui.count("[data-t=roof] .aio-chart__tip"), 0, "no point near the pointer");
    ui.hover("dots");
    ui.within("dots", |ui| ui.until_text("chartTip", "1: 1"));
    // To 10 000 points and back: the 5-point redraw is the round trip's own cost.
    let (mut big, mut small) = (vec![], vec![]);
    for _ in 0..6 {
        for times in [&mut big, &mut small] {
            let t0 = std::time::Instant::now();
            ui.click("many");
            ui.until("drawn", || true);
            times.push(t0.elapsed().as_secs_f64() * 1e3);
        }
    }
    big.sort_by(f64::total_cmp);
    small.sort_by(f64::total_cmp);
    eprintln!(
        "[bench] line chart redrawn, click to painted (median ms): 10 000 points {:.1}, 5 points {:.1}",
        big[3], small[3]
    );
    ui.click("many");
    let d = ui.eval("document.querySelector('[data-t=chart] path').getAttribute('d').length").as_u64().unwrap();
    assert!(d < 60_000, "10 000 points thinned to the plot's pixels: {d} bytes of path");
}

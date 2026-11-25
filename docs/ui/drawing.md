# Charts, heat maps and drawing

Three kit components (`aio::kit`, feature `draw`) draw data with no
JavaScript in the app, the same page code on the desktop, Android and the
web:

| Need | Use | Drawn as |
|---|---|---|
| lines, bars, dots; linear, log or category axes | `Chart` | SVG: crisp, themed, text selectable |
| a grid of values as colours (thousands of cells) | `Heatmap` | one canvas bitmap |
| anything else 2-D (a gauge, a diagram, a sprite) | `Canvas` + `Draw` | a canvas, from a list made in Rust |

They come with aio-rs's `draw` feature (it brings `kit`), so an app that
draws nothing compiles none of them, not even in a debug build:

```toml
aio = { package = "aio-rs", git = "https://github.com/riagentic/aio-rs", features = ["draw"] }
```

A release links only what it shows: the painter's script is in the binary
only when `Canvas` (or `Heatmap`) is.

## Charts

```rust
use aio::kit::{Axis, Chart, Series};
use aio::ui::prelude::*;

#[component]
fn Charts() -> View {
    let load: Vec<(f64, f64)> = (0..100).map(|i| (i as f64, (i % 10) as f64)).collect();
    rsx! {
        // A whole chart in one line.
        Chart { "data-t": "load", series: vec![Series::line("load", load)], title: "Load" }
        // Log-log, two series: a legend, ticks at powers of ten.
        Chart { "data-t": "roof", title: "Roofline",
            series: vec![
                Series::line("peak", vec![(0.1, 50.0), (20.0, 9750.0), (1000.0, 9750.0)]),
                Series::scatter("kernels", vec![(0.25, 80.0), (8.0, 3000.0)]),
            ],
            x: Axis::log("FLOP/byte"), y: Axis::log("GFLOP/s"),
        }
        // Bars by category, two series side by side.
        Chart { "data-t": "bars", height: 180.0,
            series: vec![
                Series::bar("fp32", vec![(0.0, 31.0), (1.0, 18.0)]),
                Series::bar("fp16", vec![(0.0, 58.0), (1.0, 33.0)]).color("var(--aio-accent)"),
            ],
            x: Axis::names("kernel", ["gemm", "conv"]), y: Axis::linear("TFLOP/s").range(0.0, 60.0),
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Charts);
assert_eq!(ui.count("[data-t=roof] .aio-chart__legend > span"), 2);
assert_eq!(ui.count("[data-t=load] .aio-chart__legend"), 0, "one series: no legend");
assert!(ui.text("bars").contains("gemm"));
# }
```

- `Series::line`, `::bar`, `::scatter` (`name`, `points: Vec<(f64, f64)>`),
  `.color(css)`; unset, series take `--aio-series-1` … `-8` in order (a
  palette checked for colour-blind eyes, light and dark). A ninth series
  repeats the first colour: fold small ones into "other", or draw small
  multiples.
- `Axis::linear(label)`, `Axis::log(label)`, `Axis::names(label, names)`
  (x = 0, 1, 2 … shown by name), `.range(min, max)`. Unset ends follow the
  data: y out to round ticks (and to 0 under bars), x to the data, a log
  axis to whole powers of ten. Ticks are 1, 2 or 5 × 10ⁿ; numbers read
  short (`1.5k`, `2M`, `0.001`: `aio::fmt::short`).
- Non-finite points are skipped (a line breaks there), and points ≤ 0 on a
  log axis.
- **Hover**: the nearest point (within 32 px) is ringed and its series, x
  and y show over it (`chartTip`).
- **Size**: as wide as its box, `height` px tall (240). It lays out in real
  pixels for its width, so text stays its size.
- **Accessible**: `role="img"`, `title` as its name (show a heading of
  your own), and a description a screen reader hears: each series' kind,
  count and ranges.
- **Many points**: a line of more points than its plot has pixel columns
  keeps each column's first, lowest, highest and last point: the same
  picture, a path of a few thousand points instead of 100 000. Its x must
  only grow; otherwise it is drawn as given.

## Heat maps

```rust
use aio::kit::{Heatmap, Palette};
use aio::ui::prelude::*;

#[component]
fn Occupancy() -> View {
    let sel = signal(|| None::<(usize, usize)>);
    let data: Vec<f32> = (0..64 * 64).map(|i| ((i % 64) * (i / 64)) as f32).collect();
    rsx! {
        Heatmap { "data-t": "occ", rows: 64, cols: 64, data: data, palette: Palette::Magma,
            sel: sel.get(), onpick: move |rc| sel.set(Some(rc)),
            row_labels: (0..64).map(|r| format!("SM {r}")).collect::<Vec<_>>(),
        }
        p { "data-t": "picked", {format!("{:?}", sel.get())} }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Occupancy);
ui.click_at("heatmapCells", 0.01, 0.01);
ui.until_text("picked", "Some((0, 0))");
# }
```

- `rows`, `cols`, `data` (row-major `f32`; NaN or missing cells show
  empty), `range` (unset: the data's lowest to highest).
- `palette`: `Viridis` (the default), `Magma`, `Cividis` (the same to most
  colour-blind eyes), `CoolWarm` (diverging, for values around a centre),
  or `Palette::Stops(vec![[r, g, b], …])`. `Palette::color(t)` and
  `css_gradient()` give its colours to the app's own drawing.
- `sel` outlines a cell; `onpick((row, col))` fires on a click, and on
  Enter once the arrow keys (Home, End) moved to a cell (`heatmapCells`
  takes the focus). The hovered or chosen cell's labels and value show
  over it (`heatmapTip`, a live region).
- `row_labels`, `col_labels`: as many as fit show (every 1st, 2nd, 5th,
  10th …); unset, the numbers. A colour key with the range sits under it.
- `height` (240) is the cells' box; it is as wide as its own.
- The cells are one bitmap of one byte each through a 255-colour
  palette: a 256 × 256 update sends 87 KB and paints in under 1 ms (one
  blit). The labels, key, outline and value are elements over it: a
  hover never repaints the cells.

## Drawing: `Canvas`

```rust
use aio::kit::{Align, Canvas, CanvasPointer, Draw, PointerKind};
use aio::ui::prelude::*;
use std::f64::consts::TAU;

#[component]
fn Gauge() -> View {
    let level = signal(|| 0.6);
    let draw = memo(move || {
        let mut d = Draw::new();
        d.stroke("var(--aio-border)", 10.0).arc(100.0, 60.0, 40.0, 0.0, TAU).stroke_path();
        d.stroke("var(--aio-accent)", 10.0).arc(100.0, 60.0, 40.0, -TAU / 4.0, -TAU / 4.0 + TAU * level.get()).stroke_path();
        d.fill("var(--aio-text)").align(Align::Center).text(100.0, 60.0, &format!("{:.0}%", level.get() * 100.0));
        d
    });
    rsx! {
        Canvas { "data-t": "gauge", draw: draw, view: Some((200.0, 120.0)), style: "width: 200px; height: 120px",
            onpointer: move |p: CanvasPointer| if p.kind == PointerKind::Click { level.set(p.x / 200.0) },
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Gauge);
ui.click_at("gauge", 0.25, 0.5);
assert!(ui.eval("document.querySelector('[data-t=gauge] canvas').aioPaintMs >= 0").as_bool().unwrap());
# }
```

`Draw` is the page's `CanvasRenderingContext2D` by the same names, as a
list made in Rust; each call returns the list, so calls chain:

| Group | Calls |
|---|---|
| style | `fill(css)`, `stroke(css, width)`, `alpha(a)`, `font(css)`, `align(Align)`, `dash(&[on, off])`, `smooth(bool)` |
| shapes | `rect`, `stroke_rect`, `line(x1, y1, x2, y2)`, `polyline(&[(x, y)])`, `text(x, y, s)` |
| paths | `move_to`, `line_to`, `curve_to`, `arc`, `close`, then `fill_path` or `stroke_path` |
| images | `image(src, x, y, w, h)` (an asset, a `data:` URL; painted once loaded), `pixels(&Pixels, x, y, w, h)` |
| transforms | `save`, `restore`, `translate`, `scale`, `rotate` |

- **Colours** are any CSS colour, `var(--aio-accent)` too: the painter
  reads a token as the page has it now, and paints again when the theme
  changes (a switch, the OS going dark).
- **Pixels** made in the app: `Pixels::rgba(w, h, &bytes)`, or
  `Pixels::indexed(w, h, &index, &palette)` (a byte a pixel through up to
  256 RGBA colours: a quarter of the bytes). The page decodes them once
  and keeps them while they are in the list, so a repaint is a blit.
- **Coordinates** are CSS pixels from the top left, or the `view`'s
  (`Some((w, h))`: the drawing stretched over the box, as an SVG
  `viewBox` with `preserveAspectRatio="none"`; a grid drawn in cell units).
- **Repaints**: a new list, a new size of the box, a new pixel ratio (a
  zoom, another screen) or theme; at most once a frame, at the device's
  pixel ratio, so lines stay sharp.
- **Pointer**: `onpointer(CanvasPointer { kind, x, y, buttons })` in the
  drawing's coordinates: `Down` (the canvas keeps the pointer until `Up`),
  `Move` (one a frame at most), `Up`, `Leave`, `Click`. Unset, the page
  sends none. `onsize((w, h))` tells the box's size, to lay out for it.
  On a touch screen a drag over it scrolls the page, as anywhere; for
  drawing with a finger give it `style: "touch-action: none"`.
- **Size**: it fills its box (size it with `class` or `style`); a box
  nothing sizes is 150 px tall, as a `<canvas>` is.
- `canvas.aioPaintMs` in the page is the last paint's time (ms), for
  profiling.

The painter is a kit [island](islands.md): the list goes to the page as
the island's props, its events come back through `onevent`.

## Measured

On one desktop (x86-64 Linux), the release build, the pinned Electron
offscreen; the kit's tests print theirs (`cargo test -p aio-rs --features
kit --test kit -- heatmap chart --nocapture`, lines `[bench]`):

| What | Time |
|---|---|
| a 256 × 256 heat map's new data, in the app (palette, encoding) | 0.3 ms |
| the same, in the page (decode, palette, one blit) | 0.4 ms |
| a 10 000-point line chart laid out, in the app | 0.4 ms |
| the same, in the page (its path, thinned to 23 KB, parsed and laid out) | 0.1 ms |

## Common mistakes

- A `Canvas` in a box nothing sizes, inside a flex row that shrinks it
  to 0 wide: give the box a width.
- A new `Draw` on every render of an unrelated signal: make it in a
  `memo` that reads only what it draws.
- Text drawn with a `view` that stretches unevenly: it stretches too.
  Draw labels as elements beside the canvas (as `Heatmap` does), or in
  CSS pixels.
- A chart of one value or a ratio: a `Progress` or a number reads better.
- Two measures of different units on one chart: draw two charts.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`, module `aio::kit`:
`Chart`, `Series`, `Axis`, `Scale`, `Mark`, `Heatmap`, `Palette`,
`Canvas`, `Draw`, `Pixels`, `CanvasPointer`, `PointerKind`, `Align`.

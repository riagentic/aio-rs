//! `Chart`: lines, bars and dots on linear or log axes, as SVG: crisp at
//! any size, themed by the kit's tokens (`--aio-series-1` … `-8`,
//! `--aio-border`, `--aio-muted`), its text selectable. Ticks fall on round
//! numbers; a line of more points than its width has pixels is thinned to
//! what those pixels show (each column's first, lowest, highest and last).
use crate::fmt::short;
use crate::ui::prelude::*;

/// How a series is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mark {
    /// A line through its points, in order.
    #[default]
    Line,
    /// A bar from the axis to each point; bar series side by side.
    Bar,
    /// A dot at each point.
    Scatter,
}

/// One named series of `(x, y)` points. Non-finite points are left out (a
/// line breaks there), and so are points ≤ 0 on a log axis.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub name: String,
    pub points: Vec<(f64, f64)>,
    pub mark: Mark,
    /// A CSS colour; unset: the next of the theme's `--aio-series-N`.
    pub color: Option<String>,
}

impl Series {
    fn of(mark: Mark, name: impl Into<String>, points: impl Into<Vec<(f64, f64)>>) -> Self {
        Self { name: name.into(), points: points.into(), mark, color: None }
    }

    /// A line through `points`.
    pub fn line(name: impl Into<String>, points: impl Into<Vec<(f64, f64)>>) -> Self {
        Self::of(Mark::Line, name, points)
    }

    /// Bars at `points`.
    pub fn bar(name: impl Into<String>, points: impl Into<Vec<(f64, f64)>>) -> Self {
        Self::of(Mark::Bar, name, points)
    }

    /// Dots at `points`.
    pub fn scatter(name: impl Into<String>, points: impl Into<Vec<(f64, f64)>>) -> Self {
        Self::of(Mark::Scatter, name, points)
    }

    /// Drawn in `css` (`"var(--aio-accent)"`, `"#e34948"`).
    pub fn color(self, css: impl Into<String>) -> Self {
        Self { color: Some(css.into()), ..self }
    }
}

/// An axis's scale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scale {
    #[default]
    Linear,
    /// Base 10: ticks at powers of ten (and 2 and 5 between them when it
    /// spans one).
    Log,
}

/// An axis: its label, scale and range. Unset ends follow the data (a
/// linear y axis out to round ticks, and to 0 under bars; a log axis to
/// whole powers of ten).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Axis {
    pub label: String,
    pub scale: Scale,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Names for x = 0, 1, 2 …, shown in place of numbers (bars by category).
    pub names: Vec<String>,
}

impl Axis {
    /// A linear axis labelled `label`.
    pub fn linear(label: impl Into<String>) -> Self {
        Self { label: label.into(), ..Self::default() }
    }

    /// A log axis labelled `label`.
    pub fn log(label: impl Into<String>) -> Self {
        Self { label: label.into(), scale: Scale::Log, ..Self::default() }
    }

    /// Categories: x = 0, 1, 2 … shown as `names`.
    pub fn names(label: impl Into<String>, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self { label: label.into(), names: names.into_iter().map(Into::into).collect(), ..Self::default() }
    }

    /// From `min` to `max`, whatever the data.
    pub fn range(self, min: f64, max: f64) -> Self {
        Self { min: Some(min), max: Some(max), ..self }
    }
}

/// The round step nearest `raw`: 1, 2 or 5 × 10ⁿ (pure).
fn nice(raw: f64) -> f64 {
    let m = 10f64.powf(raw.log10().floor());
    let e = raw / m;
    m * if e >= 50f64.sqrt() {
        10.0
    } else if e >= 10f64.sqrt() {
        5.0
    } else if e >= 2f64.sqrt() {
        2.0
    } else {
        1.0
    }
}

/// An axis's ends and ticks, about `room` of them.
#[derive(Clone, Debug, PartialEq)]
struct Span {
    lo: f64,
    hi: f64,
    log: bool,
    ticks: Vec<f64>,
}

impl Span {
    /// Where `v` falls, 0 at `lo` to 1 at `hi`.
    fn at(&self, v: f64) -> f64 {
        let f = |v: f64| if self.log { v.log10() } else { v };
        (f(v) - f(self.lo)) / (f(self.hi) - f(self.lo))
    }
}

/// The span of `vals` on `axis` (pure): `round` takes the ends out to
/// ticks, `zero` takes in 0, `pad` widens each side (half a bar's slot).
fn span(axis: &Axis, vals: impl Iterator<Item = f64>, room: f64, round: bool, zero: bool, pad: f64) -> Span {
    let log = axis.scale == Scale::Log;
    let ok = |v: &f64| v.is_finite() && (!log || *v > 0.0);
    let (mut lo, mut hi) =
        vals.filter(ok).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
    if lo > hi {
        (lo, hi) = if log { (1.0, 10.0) } else { (0.0, 1.0) };
    }
    if zero && !log {
        (lo, hi) = (lo.min(0.0), hi.max(0.0));
    }
    if !axis.names.is_empty() {
        (lo, hi) = (lo.min(0.0), hi.max(axis.names.len() as f64 - 1.0));
    }
    (lo, hi) = (lo - pad, hi + pad);
    let (lo, hi) = (axis.min.filter(ok).unwrap_or(lo), axis.max.filter(ok).unwrap_or(hi));
    let room = room.max(2.0);
    if log {
        let (a, b) = (lo.log10().floor(), hi.log10().ceil().max(lo.log10().floor() + 1.0));
        let (a, b) = (if axis.min.is_some() { lo.log10() } else { a }, if axis.max.is_some() { hi.log10() } else { b });
        let decades = (b.ceil() - a.floor()) as i32;
        let each: &[f64] = if decades < 2 { &[1.0, 2.0, 5.0] } else { &[1.0] };
        // Powers of ten are short and evenly spaced: twice as many fit.
        let stride = ((decades as f64 / (room * 2.0)).ceil() as i32).max(1);
        let ticks = (a.floor() as i32..=b.ceil() as i32)
            .filter(|d| d.rem_euclid(stride) == 0)
            .flat_map(|d| each.iter().map(move |m| m * 10f64.powi(d)))
            .filter(|t| t.log10() >= a - 1e-9 && t.log10() <= b + 1e-9)
            .collect();
        return Span { lo: 10f64.powf(a), hi: 10f64.powf(b), log, ticks };
    }
    let (lo, hi) = if hi > lo { (lo, hi) } else { (lo - 1.0, hi + 1.0) };
    let step = if axis.names.is_empty() { nice((hi - lo) / room) } else { nice((hi - lo) / room).max(1.0) };
    let (lo, hi) = match round {
        true => (
            if axis.min.is_some() { lo } else { (lo / step).floor() * step },
            if axis.max.is_some() { hi } else { (hi / step).ceil() * step },
        ),
        false => (lo, hi),
    };
    let ticks =
        ((lo / step - 1e-9).ceil() as i64..=(hi / step + 1e-9).floor() as i64).map(|k| k as f64 * step).collect();
    Span { lo, hi, log, ticks }
}

/// A tick's text: its name on a category axis, else its number.
fn tick_text(axis: &Axis, v: f64) -> String {
    match axis.names.get(v.round() as usize) {
        Some(n) if !axis.names.is_empty() && (v - v.round()).abs() < 1e-9 && v >= 0.0 => n.clone(),
        _ if !axis.names.is_empty() => String::new(),
        _ => short(v),
    }
}

/// A line's points, thinned to the first, lowest, highest and last of each
/// pixel column when there are many more points than columns and x only
/// grows (pure): what it draws is the same.
fn thin(pts: &[(f64, f64)], px: impl Fn(f64) -> f64) -> Vec<(f64, f64)> {
    let sorted = pts.windows(2).all(|w| w[0].0 <= w[1].0 || !(w[0].0.is_finite() && w[1].0.is_finite()));
    if pts.len() < 4096 || !sorted {
        return pts.to_vec();
    }
    let mut out = Vec::new();
    let mut col: Vec<(f64, f64)> = Vec::new();
    let flush = |col: &mut Vec<(f64, f64)>, out: &mut Vec<(f64, f64)>| {
        if let (Some(&first), Some(&last)) = (col.first(), col.last()) {
            let lo = col.iter().copied().fold(first, |a, p| if p.1 < a.1 { p } else { a });
            let hi = col.iter().copied().fold(first, |a, p| if p.1 > a.1 { p } else { a });
            let mut four = [first, lo, hi, last];
            four.sort_by(|a, b| a.0.total_cmp(&b.0));
            for p in four {
                if out.last() != Some(&p) {
                    out.push(p);
                }
            }
        }
        col.clear();
    };
    let mut at = f64::NAN;
    for &p in pts {
        if !(p.0.is_finite() && p.1.is_finite()) {
            flush(&mut col, &mut out);
            out.push(p);
            continue;
        }
        let c = px(p.0).floor();
        if c != at {
            flush(&mut col, &mut out);
            at = c;
        }
        col.push(p);
    }
    flush(&mut col, &mut out);
    out
}

/// A series' colour.
fn color(s: &Series, i: usize) -> String {
    const FALLBACK: [&str; 8] =
        ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948"];
    s.color.clone().unwrap_or_else(|| format!("var(--aio-series-{}, {})", i % 8 + 1, FALLBACK[i % 8]))
}

/// A legend swatch's style: a stroke for a line, a square for the rest.
fn swatch(s: &Series, i: usize) -> String {
    let c = color(s, i);
    match s.mark {
        Mark::Line => format!("width: 14px; height: 2px; background: {c}"),
        _ => format!("width: 10px; height: 10px; border-radius: 2px; background: {c}"),
    }
}

/// Everything drawn, in pixels of a `w` × `h` box (pure).
#[derive(Clone, Debug, PartialEq)]
struct Plot {
    w: f64,
    h: f64,
    /// Along y: (y, left, right, text); along x: (x, bottom, text).
    yt: Vec<(f64, f64, f64, String)>,
    xt: Vec<(f64, f64, String)>,
    /// The axes' labels: (transform, text).
    labels: Vec<(String, String)>,
    /// Per series: its path and style.
    paths: Vec<(String, String)>,
    /// What the series may cover (the plot area and half a dot more): x, y, w, h.
    clip: (f64, f64, f64, f64),
    /// Per series: its points in pixels, as drawn (for the hover).
    at: Vec<Vec<(f64, f64, f64, f64)>>,
}

fn plot(series: &[Series], x: &Axis, y: &Axis, w: f64, h: f64) -> Plot {
    let bars = series.iter().filter(|s| s.mark == Mark::Bar).count();
    let xs = || series.iter().flat_map(|s| s.points.iter().map(|p| p.0));
    let ys = || series.iter().flat_map(|s| s.points.iter().map(|p| p.1));
    let ysp = span(y, ys(), (h - 40.0) / 50.0, true, bars > 0, 0.0);
    let yt: Vec<String> = ysp.ticks.iter().map(|&v| tick_text(y, v)).collect();
    let widest = yt.iter().map(|t| t.chars().count()).max().unwrap_or(1) as f64;
    // Margins: left, right, top, bottom.
    let m = (
        10.0 + widest * 6.5 + if y.label.is_empty() { 0.0 } else { 18.0 },
        16.0,
        10.0,
        24.0 + if x.label.is_empty() { 0.0 } else { 18.0 },
    );
    let (pw, ph) = ((w - m.0 - m.1).max(1.0), (h - m.2 - m.3).max(1.0));
    // Bars: half the closest two x's apart on each side.
    let pad = match bars > 0 && x.scale == Scale::Linear {
        true => {
            let mut v: Vec<f64> = series
                .iter()
                .filter(|s| s.mark == Mark::Bar)
                .flat_map(|s| s.points.iter().map(|p| p.0))
                .filter(|v| v.is_finite())
                .collect();
            v.sort_by(f64::total_cmp);
            v.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 0.0).reduce(f64::min).unwrap_or(1.0) / 2.0
        }
        false => 0.0,
    };
    let xsp = span(x, xs(), pw / 100.0, false, false, pad);
    let px = |v: f64| m.0 + xsp.at(v) * pw;
    let py = |v: f64| m.2 + (1.0 - ysp.at(v)) * ph;
    let usable = |(a, b): (f64, f64)| {
        a.is_finite() && b.is_finite() && (x.scale != Scale::Log || a > 0.0) && (y.scale != Scale::Log || b > 0.0)
    };
    // Bars: each x's slot, shared by the bar series side by side.
    let gap = series
        .iter()
        .filter(|s| s.mark == Mark::Bar)
        .flat_map(|s| {
            let mut v: Vec<f64> = s.points.iter().map(|p| px(p.0)).filter(|v| v.is_finite()).collect();
            v.sort_by(f64::total_cmp);
            v.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 0.5).collect::<Vec<_>>()
        })
        .fold(pw / 2.0, f64::min);
    let slot = gap * 0.8 / bars.max(1) as f64;
    let base = py(if y.scale == Scale::Log { ysp.lo } else { 0f64.clamp(ysp.lo, ysp.hi) });
    let mut nbar = 0;
    let mut paths = Vec::new();
    let mut at = Vec::new();
    for (i, s) in series.iter().enumerate() {
        let pts: Vec<(f64, f64)> = match s.mark {
            Mark::Line => thin(&s.points, px),
            _ => s.points.clone(),
        };
        let mut d = String::with_capacity(pts.len() * 14);
        let mut hits = Vec::new();
        let mut pen = false;
        let k = nbar as f64;
        for p in pts {
            if !usable(p) {
                pen = false;
                continue;
            }
            let (a, b) = (px(p.0), py(p.1));
            use std::fmt::Write;
            match s.mark {
                Mark::Line => _ = write!(d, "{}{a:.1} {b:.1}", if pen { "L" } else { "M" }),
                Mark::Scatter => _ = write!(d, "M{a:.1} {b:.1}h0"),
                Mark::Bar => {
                    let x0 = a - gap * 0.4 + k * slot + 1.0;
                    _ = write!(d, "M{x0:.1} {base:.1}V{b:.1}h{:.1}V{base:.1}Z", (slot - 2.0).max(1.0));
                    hits.push((x0 + (slot - 2.0).max(1.0) / 2.0, b, p.0, p.1));
                    continue;
                }
            }
            pen = true;
            hits.push((a, b, p.0, p.1));
        }
        if s.mark == Mark::Bar {
            nbar += 1;
        }
        let c = color(s, i);
        let style = match s.mark {
            Mark::Line => {
                format!("fill: none; stroke: {c}; stroke-width: 2; stroke-linejoin: round; stroke-linecap: round")
            }
            Mark::Scatter => format!("fill: none; stroke: {c}; stroke-width: 8; stroke-linecap: round"),
            Mark::Bar => format!("fill: {c}"),
        };
        paths.push((d, style));
        at.push(hits);
    }
    let bottom = h - m.3;
    let xt = xsp.ticks.iter().map(|&v| (px(v), bottom, tick_text(x, v))).filter(|t| !t.2.is_empty()).collect();
    let yt = ysp.ticks.iter().zip(yt).map(|(&v, t)| (py(v), m.0, w - m.1, t)).collect();
    let mut labels = vec![];
    if !x.label.is_empty() {
        labels.push((format!("translate({:.1} {:.1})", m.0 + pw / 2.0, h - 8.0), x.label.clone()));
    }
    if !y.label.is_empty() {
        labels.push((format!("translate(10 {:.1}) rotate(-90)", m.2 + ph / 2.0), y.label.clone()));
    }
    Plot { w, h, xt, yt, labels, paths, at, clip: (m.0 - 6.0, m.2 - 6.0, pw + 12.0, ph + 12.0) }
}

/// The point nearest `(x, y)` within 32 px: its series and point.
fn nearest(p: &Plot, x: f64, y: f64) -> Option<(usize, (f64, f64, f64, f64))> {
    p.at.iter()
        .enumerate()
        .flat_map(|(i, s)| s.iter().map(move |q| (i, *q)))
        .map(|(i, q)| ((q.0 - x).powi(2) + (q.1 - y).powi(2), i, q))
        .filter(|(d, ..)| *d <= 32.0 * 32.0)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, i, q)| (i, q))
}

/// What a screen reader hears of it (pure).
fn describe(series: &[Series], x: &Axis, y: &Axis) -> String {
    let range = |v: &mut dyn Iterator<Item = f64>| {
        let (lo, hi) =
            v.filter(|v| v.is_finite()).fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(v), b.max(v)));
        if lo <= hi { format!("{} to {}", short(lo), short(hi)) } else { "none".into() }
    };
    let name = |a: &Axis, s: &str| if a.label.is_empty() { s.to_string() } else { a.label.clone() };
    series
        .iter()
        .map(|s| {
            let kind = ["line", "bars", "dots"][s.mark as usize];
            format!(
                "{} ({kind}): {} points, {} {}, {} {}.",
                s.name,
                s.points.len(),
                name(x, "x"),
                range(&mut s.points.iter().map(|p| p.0)),
                name(y, "y"),
                range(&mut s.points.iter().map(|p| p.1))
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A chart of `series` (each a line, bars or dots) on axes `x` and `y`
/// (linear, log, or categories), with ticks at round numbers, a legend
/// for two series or more, and the nearest point's values on hover.
/// `title` is its accessible name (show a heading of your own); a screen
/// reader also hears each series' range. As wide as its box, `height`
/// px tall (240).
///
/// `Chart { series: vec![Series::line("load", points)] }` is a whole chart.
#[component]
pub fn Chart(
    series: Prop<Vec<Series>>,
    #[props(default)] x: Prop<Axis>,
    #[props(default)] y: Prop<Axis>,
    #[props(default, into)] title: Prop<String>,
    #[props(default = 240.0)] height: Prop<f64>,
    #[props(default, into)] class: Prop<String>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    let id = super::use_id("chart");
    let width = signal(|| 640.0);
    let drawn = memo(move || series.with(|s| x.with(|x| y.with(|y| plot(s, x, y, width.get(), height.get())))));
    let hover = signal(|| None::<(f64, f64)>);
    let near = memo(move || hover.get().and_then(|(a, b)| drawn.with(|p| nearest(p, a, b))));
    let desc = memo(move || series.with(|s| x.with(|x| y.with(|y| describe(s, x, y)))));
    let legend = memo(move || {
        series.with(|s| match s.len() {
            0 | 1 => vec![],
            _ => s.iter().enumerate().map(|(i, s)| (s.name.clone(), swatch(s, i))).collect(),
        })
    });
    let tip = memo(move || {
        near.get().map(|(i, (a, b, vx, vy))| {
            let (name, xs) = series.with(|s| (s.get(i).map(|s| s.name.clone()).unwrap_or_default(), s.len()));
            let xv = x.with(|x| match tick_text(x, vx) {
                t if t.is_empty() => short(vx),
                t => t,
            });
            let lead = if xs > 1 { format!("{name} · ") } else { String::new() };
            (a, drawn.with(|p| a / p.w * 100.0), b, format!("{lead}{xv}: {}", short(vy)))
        })
    });
    let (tid, did, cid) = (format!("{id}-t"), format!("{id}-d"), format!("{id}-c"));
    let name = memo(move || if title.get().is_empty() { "Chart".to_string() } else { title.get() });
    let ink = "fill: var(--aio-muted, currentColor); font-size: 11px";
    let rule = "stroke: var(--aio-border, #8884); stroke-width: 1";
    rsx! {
        div {
            class: super::join("aio-chart", &class.get()),
            onresize: move |e: ResizeEvent| {
                if let Ok(s) = e.get_border_box_size() && s.width > 0.0 {
                    width.set(s.width)
                }
            },
            ..attributes,
            if !legend.get().is_empty() {
                div { class: "aio-chart__legend", style: "display: flex; flex-wrap: wrap; gap: 4px 14px; font-size: .8em; margin-bottom: 4px",
                    for (i, (name, swatch)) in legend.get().into_iter().enumerate() {
                        span { key: "{i}", style: "display: inline-flex; align-items: center; gap: 6px",
                            span { "aria-hidden": "true", style: "{swatch}" }
                            "{name}"
                        }
                    }
                }
            }
            div { style: "position: relative",
                svg {
                    role: "img",
                    "aria-labelledby": "{tid} {did}",
                    width: "100%",
                    height: height.get(),
                    view_box: drawn.with(|p| format!("0 0 {} {}", p.w, p.h)),
                    style: "display: block; overflow: visible",
                    onpointermove: move |e: PointerEvent| {
                        let p = e.element_coordinates();
                        hover.set(Some((p.x, p.y)))
                    },
                    onpointerleave: move |_| hover.set(None),
                    title { id: "{tid}", "{name}" }
                    clipPath { id: "{cid}",
                        rect { x: drawn.with(|p| p.clip.0), y: drawn.with(|p| p.clip.1), width: drawn.with(|p| p.clip.2), height: drawn.with(|p| p.clip.3) }
                    }
                    desc { id: "{did}", "{desc}" }
                    g { style: "pointer-events: none",
                        for (i, (at, x0, x1, text)) in drawn.with(|p| p.yt.clone()).into_iter().enumerate() {
                            g { key: "{i}",
                                line { x1: "{x0}", x2: "{x1}", y1: "{at:.1}", y2: "{at:.1}", style: rule }
                                text { x: "{x0 - 6.0}", y: "{at:.1}", "text-anchor": "end", "dominant-baseline": "middle", style: ink, "{text}" }
                            }
                        }
                        for (i, (at, y0, text)) in drawn.with(|p| p.xt.clone()).into_iter().enumerate() {
                            g { key: "{i}",
                                line { x1: "{at:.1}", x2: "{at:.1}", y1: "{y0}", y2: "{y0 + 4.0}", style: rule }
                                text { x: "{at:.1}", y: "{y0 + 16.0}", "text-anchor": "middle", style: ink, "{text}" }
                            }
                        }
                        for (i, (transform, text)) in drawn.with(|p| p.labels.clone()).into_iter().enumerate() {
                            text { key: "{i}", transform: "{transform}", "text-anchor": "middle", "dominant-baseline": "middle", style: ink, "{text}" }
                        }
                        g { "clip-path": "url(#{cid})",
                            for (i, (d, style)) in drawn.with(|p| p.paths.clone()).into_iter().enumerate() {
                                path { key: "{i}", d: "{d}", style: "{style}" }
                            }
                        }
                        if let Some((a, _, b, _)) = tip.get() {
                            circle { cx: "{a:.1}", cy: "{b:.1}", r: "5", style: "fill: none; stroke: var(--aio-text, CanvasText); stroke-width: 2" }
                        }
                    }
                }
                if let Some((_, a, b, text)) = tip.get() {
                    div { class: "aio-chart__tip", "data-t": "chartTip",
                        style: "position: absolute; pointer-events: none; left: {a}%; top: {b}px; transform: translate(-50%, calc(-100% - 10px)); padding: 2px 6px; border-radius: 4px; white-space: nowrap; font-size: .8em; background: var(--aio-text, CanvasText); color: var(--aio-bg, Canvas)",
                        "{text}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticks(a: &Axis, v: &[f64], room: f64, round: bool) -> (f64, f64, Vec<f64>) {
        let s = span(a, v.iter().copied(), room, round, false, 0.0);
        (s.lo, s.hi, s.ticks)
    }

    #[test]
    fn steps_are_the_nearest_round_numbers() {
        let got: Vec<f64> = [0.9, 1.2, 1.5, 3.0, 4.0, 7.0, 7.2, 0.023].into_iter().map(nice).collect();
        assert_eq!(got, [1.0, 1.0, 2.0, 2.0, 5.0, 5.0, 10.0, 0.02]);
    }

    #[test]
    fn a_linear_axis_ends_on_ticks_when_asked_and_a_log_axis_on_decades() {
        let lin = Axis::default();
        assert_eq!(ticks(&lin, &[0.3, 9.2], 5.0, true), (0.0, 10.0, vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0]));
        assert_eq!(ticks(&lin, &[0.3, 9.2], 5.0, false).2, [2.0, 4.0, 6.0, 8.0], "x stays on the data");
        assert_eq!(ticks(&lin, &[5.0, 5.0], 2.0, true).0, 4.0, "one value: a span around it");
        assert_eq!(
            ticks(&lin.clone().range(-1.0, 1.0), &[0.0, 50.0], 4.0, true),
            (-1.0, 1.0, vec![-1.0, -0.5, 0.0, 0.5, 1.0])
        );
        let log = Axis::log("");
        assert_eq!(
            ticks(&log, &[0.5, 900.0, -3.0, 0.0], 4.0, true),
            (0.1, 1000.0, vec![0.1, 1.0, 10.0, 100.0, 1000.0]),
            "≤ 0 left out"
        );
        assert_eq!(ticks(&log, &[2.0, 8.0], 4.0, true).2, [1.0, 2.0, 5.0, 10.0], "one decade: 2 and 5 too");
        assert_eq!(
            ticks(&log, &[1e-6, 1e6], 3.0, true).2,
            [1e-6, 1e-4, 0.01, 1.0, 100.0, 1e4, 1e6],
            "many: every few decades"
        );
        assert_eq!(ticks(&log, &[], 3.0, true), (1.0, 10.0, vec![1.0, 2.0, 5.0, 10.0]), "no data: one decade");
    }

    #[test]
    fn a_category_axis_names_its_whole_ticks() {
        let a = Axis::names("", ["a", "b", "c"]);
        let s = span(&a, [0.0, 2.0].into_iter(), 10.0, false, false, 0.5);
        assert_eq!((s.lo, s.hi), (-0.5, 2.5), "half a slot each side");
        let t: Vec<String> = s.ticks.iter().map(|&v| tick_text(&a, v)).filter(|t| !t.is_empty()).collect();
        assert_eq!(t, ["a", "b", "c"]);
        assert_eq!(tick_text(&Axis::default(), 1500.0), "1.5k");
    }

    #[test]
    fn a_long_line_keeps_each_pixel_columns_ends_and_extremes() {
        let pts: Vec<(f64, f64)> =
            (0..10_000).map(|i| (i as f64, if i == 5_003 { 99.0 } else { (i % 10) as f64 })).collect();
        let t = thin(&pts, |x| x / 100.0);
        assert!(t.len() <= 4 * 100, "{}", t.len());
        assert!(t.contains(&(5_003.0, 99.0)), "a spike stays");
        assert_eq!((t[0], *t.last().unwrap()), (pts[0], pts[9_999]));
        assert!(t.windows(2).all(|w| w[0].0 <= w[1].0), "still in order");
        let unsorted: Vec<(f64, f64)> = (0..5_000).map(|i| ((i % 3) as f64, 0.0)).collect();
        assert_eq!(thin(&unsorted, |x| x).len(), 5_000, "x that doesn't only grow: as it is");
    }

    #[test]
    fn a_plot_puts_points_in_its_box_and_skips_what_it_cannot_draw() {
        let s = [Series::line("a", vec![(0.0, 0.0), (f64::NAN, 1.0), (1.0, 1.0), (2.0, 2.0)])];
        let p = plot(&s, &Axis::default(), &Axis::default(), 300.0, 200.0);
        assert!(
            p.paths[0].0.starts_with('M') && p.paths[0].0.matches('M').count() == 2,
            "a gap breaks the line: {}",
            p.paths[0].0
        );
        assert!(p.at[0].iter().all(|q| q.0 >= 0.0 && q.0 <= 300.0 && q.1 >= 0.0 && q.1 <= 200.0));
        assert_eq!(nearest(&p, p.at[0][1].0 + 3.0, p.at[0][1].1).map(|n| (n.1.2, n.1.3)), Some((1.0, 1.0)));
        assert_eq!(nearest(&p, -100.0, -100.0), None);
        let b = [Series::bar("a", vec![(0.0, 3.0)]), Series::bar("b", vec![(0.0, 5.0)])];
        let p = plot(&b, &Axis::names("", ["x"]), &Axis::default(), 300.0, 200.0);
        assert!(p.at[0][0].0 < p.at[1][0].0, "bar series side by side");
        assert_eq!(p.yt[0].3, "0", "bars stand on 0");
    }

    #[test]
    fn a_screen_reader_hears_each_series_range() {
        let s = [Series::scatter("k", vec![(1.0, 2.0), (3.0, 4000.0)])];
        assert_eq!(
            describe(&s, &Axis::log("intensity"), &Axis::default()),
            "k (dots): 2 points, intensity 1 to 3, y 2 to 4k."
        );
    }
}

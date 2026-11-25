//! `Heatmap`: rows × cols of values as colours, on a [`Canvas`]: the
//! cells are one bitmap of one byte each, through a 255-colour palette, so
//! 65 536 cells are one blit. Labels, the colour key, the hover's value and
//! the selection are the page's own elements over it: crisp, themed,
//! selectable.
use super::canvas::{Canvas, CanvasPointer, Draw, Pixels, PointerKind};
use crate::ui::prelude::*;

/// A heat map's colours from low to high. The built-in ones are
/// perceptually uniform (an equal step in value looks an equal step) and
/// read in greys and by colour-blind eyes.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Palette {
    /// Dark blue through green to yellow (matplotlib's default).
    #[default]
    Viridis,
    /// Black through purple and orange to pale yellow.
    Magma,
    /// Blue to yellow, the same to most colour-blind eyes.
    Cividis,
    /// Diverging: blue, grey in the middle, red (Moreland's cool-warm):
    /// for values around a centre (a change, an error).
    CoolWarm,
    /// Evenly spaced RGB stops, low to high (at least one).
    Stops(Vec<[u8; 3]>),
}

const VIRIDIS: [u32; 9] = [0x440154, 0x472d7b, 0x3b528b, 0x2c728e, 0x21908c, 0x27ad81, 0x5dc863, 0xaadc32, 0xfde725];
const MAGMA: [u32; 9] = [0x000004, 0x1d1147, 0x51127c, 0x822681, 0xb63679, 0xe65164, 0xfb8861, 0xfec287, 0xfcfdbf];
const CIVIDIS: [u32; 9] = [0x00204d, 0x00336f, 0x39486b, 0x575c6d, 0x707173, 0x8a8779, 0xa69d75, 0xc4b56c, 0xffea46];
const COOL_WARM: [u32; 5] = [0x3b4cc0, 0x8db0fe, 0xdddddd, 0xf49a7b, 0xb40426];

impl Palette {
    fn stops(&self) -> Vec<[u8; 3]> {
        let rgb = |c: &u32| [(c >> 16) as u8, (c >> 8) as u8, *c as u8];
        match self {
            Self::Viridis => VIRIDIS.iter().map(rgb).collect(),
            Self::Magma => MAGMA.iter().map(rgb).collect(),
            Self::Cividis => CIVIDIS.iter().map(rgb).collect(),
            Self::CoolWarm => COOL_WARM.iter().map(rgb).collect(),
            Self::Stops(s) if s.is_empty() => vec![[0; 3]],
            Self::Stops(s) => s.clone(),
        }
    }

    /// The colour at `t` (0 low to 1 high, clamped), between its stops.
    pub fn color(&self, t: f32) -> [u8; 3] {
        let s = self.stops();
        let at = t.clamp(0.0, 1.0) * (s.len() - 1) as f32;
        let (i, f) = (at.floor() as usize, at.fract());
        let (a, b) = (s[i], s[(i + 1).min(s.len() - 1)]);
        std::array::from_fn(|k| (f32::from(a[k]) + (f32::from(b[k]) - f32::from(a[k])) * f).round() as u8)
    }

    /// As a CSS `linear-gradient`, left to right (the colour key's).
    pub fn css_gradient(&self) -> String {
        let stops: Vec<String> = self.stops().iter().map(|[r, g, b]| format!("rgb({r} {g} {b})")).collect();
        match &stops[..] {
            [one] => format!("linear-gradient(to right, {one}, {one})"),
            _ => format!("linear-gradient(to right, {})", stops.join(", ")),
        }
    }
}

/// Byte 255: a cell with no value (NaN, past the data's end), transparent.
const EMPTY: u8 = 255;

/// The cells as palette indexes (0 low to 254 high, [`EMPTY`] none) over
/// `lo..=hi`, rows × cols from row-major `data` (pure).
pub(crate) fn indexes(data: &[f32], rows: usize, cols: usize, (lo, hi): (f32, f32)) -> Vec<u8> {
    let k = if hi > lo { 254.0 / (hi - lo) } else { 0.0 };
    (0..rows * cols)
        .map(|i| match data.get(i) {
            Some(v) if v.is_finite() => ((v - lo) * k).round().clamp(0.0, 254.0) as u8,
            _ => EMPTY,
        })
        .collect()
}

/// The finite values' lowest and highest; `(0, 1)` with none (pure).
pub(crate) fn extent(data: &[f32]) -> (f32, f32) {
    let (lo, hi) = data
        .iter()
        .filter(|v| v.is_finite())
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    if lo <= hi { (lo, hi) } else { (0.0, 1.0) }
}

/// The drawing: the cells, one byte each, through `palette`.
fn cells(data: &[f32], rows: usize, cols: usize, range: (f32, f32), palette: &Palette) -> Draw {
    let lut: Vec<[u8; 4]> = (0..255)
        .map(|i| {
            let [r, g, b] = palette.color(i as f32 / 254.0);
            [r, g, b, 255]
        })
        .collect();
    let px = Pixels::indexed(cols as u32, rows as u32, &indexes(data, rows, cols, range), &lut);
    let mut d = Draw::new();
    d.smooth(false).pixels(&px, 0.0, 0.0, cols as f64, rows as f64);
    d
}

/// Which labels of `n` to show so at most `room` do: every 1st, 2nd,
/// 5th, 10th, 20th … (pure).
fn every(n: usize, room: usize) -> usize {
    let mut m = 1;
    loop {
        for f in [1, 2, 5] {
            if n.div_ceil(f * m) <= room.max(1) {
                return f * m;
            }
        }
        m *= 10;
    }
}

/// A cell's name: its labels' (or its numbers), and its value.
fn cell_text(rows: &[String], cols: &[String], (r, c): (usize, usize), v: Option<f32>) -> String {
    let name = |l: &[String], i: usize| l.get(i).cloned().unwrap_or_else(|| i.to_string());
    let v = v.filter(|v| v.is_finite()).map_or("no value".into(), |v| crate::fmt::short(f64::from(v)));
    format!("{}, {}: {v}", name(rows, r), name(cols, c))
}

/// `rows` × `cols` values (`data`, row-major; NaN or missing cells empty) as
/// colours of `palette` (Viridis) over `range` (unset: the data's lowest to
/// highest), with a colour key under it.
/// - `row_labels`, `col_labels`: each row's and column's name, as many as
///   fit shown beside it; unset, their numbers.
/// - `sel`: the cell outlined; `onpick`: a click on a cell, or Enter on
///   the cell the arrow keys moved to, as `(row, col)`.
/// - The hovered cell's labels and value show over it.
///
/// As wide as its box; the cells `height` px tall (240).
#[component]
pub fn Heatmap(
    rows: Prop<usize>,
    cols: Prop<usize>,
    data: Prop<Vec<f32>>,
    #[props(default)] palette: Prop<Palette>,
    #[props(default)] range: Prop<Option<(f32, f32)>>,
    #[props(default)] sel: Prop<Option<(usize, usize)>>,
    onpick: Option<EventHandler<(usize, usize)>>,
    #[props(default)] row_labels: Prop<Vec<String>>,
    #[props(default)] col_labels: Prop<Vec<String>>,
    #[props(default = 240.0)] height: Prop<f64>,
    #[props(default, into)] class: Prop<String>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    let span = memo(move || range.get().unwrap_or_else(|| data.with(|d| extent(d))));
    let draw = memo(move || {
        let (r, c) = (rows.get(), cols.get());
        data.with(|d| palette.with(|p| cells(d, r, c, span.get(), p)))
    });
    // The cell under the pointer or the keys.
    let at = signal(|| None::<(usize, usize)>);
    let cell = move |x: f64, y: f64| {
        let (r, c) = (rows.get_untracked(), cols.get_untracked());
        (x >= 0.0 && y >= 0.0 && (y as usize) < r && (x as usize) < c).then_some((y as usize, x as usize))
    };
    let pointer = move |p: CanvasPointer| match p.kind {
        PointerKind::Leave => at.set(None),
        PointerKind::Click => {
            if let (Some(rc), Some(f)) = (cell(p.x, p.y), onpick) {
                f.call(rc)
            }
        }
        _ => at.set(cell(p.x, p.y)),
    };
    let key = move |e: KeyboardEvent| {
        let (r, c) = (rows.get_untracked(), cols.get_untracked());
        if r == 0 || c == 0 {
            return;
        }
        let (y, x) = at.get_untracked().or(sel.get_untracked()).unwrap_or((0, 0));
        let to = match e.key() {
            Key::ArrowUp => (y.saturating_sub(1), x),
            Key::ArrowDown => ((y + 1).min(r - 1), x),
            Key::ArrowLeft => (y, x.saturating_sub(1)),
            Key::ArrowRight => (y, (x + 1).min(c - 1)),
            Key::Home => (y, 0),
            Key::End => (y, c - 1),
            Key::Enter => {
                if let Some(f) = onpick {
                    f.call((y.min(r - 1), x.min(c - 1)))
                }
                return e.prevent_default();
            }
            _ => return,
        };
        e.prevent_default();
        at.set(Some(to));
    };
    // A box over cell `rc`, in percents of the cells' box.
    let over = move |(r, c): (usize, usize)| {
        let (rs, cs) = (rows.get().max(1) as f64, cols.get().max(1) as f64);
        format!(
            "position: absolute; pointer-events: none; left: {}%; top: {}%; width: {}%; height: {}%",
            c as f64 * 100.0 / cs,
            r as f64 * 100.0 / rs,
            100.0 / cs,
            100.0 / rs
        )
    };
    let tip = memo(move || {
        at.get().map(|rc| {
            let v = data.with(|d| d.get(rc.0 * cols.get() + rc.1).copied());
            (rc, row_labels.with(|rl| col_labels.with(|cl| cell_text(rl, cl, rc, v))))
        })
    });
    // Labels along a side: (percent along it, text).
    let marks = move |n: usize, labels: &[String]| -> Vec<(f64, String)> {
        let step = every(n, if labels.is_empty() { 8 } else { 16 });
        (0..n)
            .step_by(step)
            .map(|i| ((i as f64 + 0.5) * 100.0 / n as f64, labels.get(i).cloned().unwrap_or_else(|| i.to_string())))
            .collect()
    };
    let row_marks = memo(move || row_labels.with(|l| marks(rows.get(), l)));
    let row_room = memo(move || row_marks.with(|m| m.iter().map(|(_, t)| t.chars().count()).max().unwrap_or(1)));
    let col_marks = memo(move || col_labels.with(|l| marks(cols.get(), l)));
    let key_bar = memo(move || palette.with(Palette::css_gradient));
    let (lo, hi) = (
        memo(move || crate::fmt::short(f64::from(span.get().0))),
        memo(move || crate::fmt::short(f64::from(span.get().1))),
    );
    let small = "font-size: .75em; color: var(--aio-muted)";
    rsx! {
        div {
            class: super::join("aio-heatmap", &class.get()),
            role: "figure",
            style: "display: grid; grid-template-columns: auto 1fr; gap: 4px 6px",
            ..attributes,
            div { style: "position: relative; min-width: {row_room}ch; {small}", "aria-hidden": "true",
                for (i, (at, text)) in row_marks.get().into_iter().enumerate() {
                    span { key: "{i}", style: "position: absolute; right: 0; top: {at}%; transform: translateY(-50%); white-space: nowrap", "{text}" }
                }
            }
            div {
                "data-t": "heatmapCells",
                style: "position: relative; height: {height}px; outline-offset: 2px",
                tabindex: "0",
                role: "img",
                "aria-label": format!("Heat map, {} rows by {} columns, values {} to {}", rows.get(), cols.get(), lo.get(), hi.get()),
                onkeydown: key,
                onblur: move |_| at.set(None),
                Canvas { draw: draw, view: Some((cols.get().max(1) as f64, rows.get().max(1) as f64)),
                    onpointer: pointer, style: "position: absolute; inset: 0; height: auto; cursor: crosshair" }
                if let Some(rc) = sel.get() {
                    div { class: "aio-heatmap__sel", style: "{over(rc)}; outline: 2px solid var(--aio-text, CanvasText); box-shadow: 0 0 0 3px var(--aio-bg, Canvas)" }
                }
                if let Some((rc, text)) = tip.get() {
                    div { style: "{over(rc)}; outline: 1px solid var(--aio-bg, Canvas)" }
                    div { class: "aio-heatmap__tip", "data-t": "heatmapTip", role: "status",
                        style: "position: absolute; pointer-events: none; z-index: 1; left: {(rc.1 as f64 + 0.5) * 100.0 / cols.get().max(1) as f64}%; top: {rc.0 as f64 * 100.0 / rows.get().max(1) as f64}%; transform: translate(-50%, calc(-100% - 6px)); padding: 2px 6px; border-radius: 4px; white-space: nowrap; font-size: .8em; background: var(--aio-text, CanvasText); color: var(--aio-bg, Canvas)",
                        "{text}"
                    }
                }
            }
            div {}
            div { style: "position: relative; height: 1.4em; {small}", "aria-hidden": "true",
                for (i, (at, text)) in col_marks.get().into_iter().enumerate() {
                    span { key: "{i}", style: "position: absolute; top: 0; left: {at}%; transform: translateX(-50%); white-space: nowrap", "{text}" }
                }
            }
            div {}
            div { style: "display: flex; align-items: center; gap: 6px; {small}", "aria-hidden": "true",
                span { "{lo}" }
                span { style: "flex: 0 1 12em; height: .6em; border-radius: 3px; background: {key_bar}" }
                span { "{hi}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_map_onto_the_palette_with_empty_cells_apart() {
        let d = [0.0, 5.0, 10.0, f32::NAN];
        assert_eq!(indexes(&d, 2, 3, (0.0, 10.0)), [0, 127, 254, 255, 255, 255], "NaN and past the end: empty");
        assert_eq!(indexes(&[-1.0, 11.0], 1, 2, (0.0, 10.0)), [0, 254], "clamped to the range");
        assert_eq!(indexes(&[3.0, 3.0], 1, 2, (3.0, 3.0)), [0, 0], "a flat range is all low");
        assert_eq!(extent(&[f32::NAN, 2.0, -1.0, f32::INFINITY]), (-1.0, 2.0));
        assert_eq!(extent(&[f32::NAN]), (0.0, 1.0));
    }

    #[test]
    fn a_palette_runs_between_its_stops() {
        assert_eq!(Palette::Viridis.color(0.0), [0x44, 0x01, 0x54]);
        assert_eq!(Palette::Viridis.color(1.0), [0xfd, 0xe7, 0x25]);
        assert_eq!(Palette::Viridis.color(7.0), [0xfd, 0xe7, 0x25], "clamped");
        let grey = Palette::Stops(vec![[0, 0, 0], [200, 100, 0]]);
        assert_eq!(grey.color(0.5), [100, 50, 0]);
        assert_eq!(Palette::Stops(vec![]).color(0.3), [0, 0, 0], "no stops: black, no panic");
        assert_eq!(Palette::Stops(vec![[9, 9, 9]]).css_gradient(), "linear-gradient(to right, rgb(9 9 9), rgb(9 9 9))");
    }

    #[test]
    fn labels_thin_to_round_steps() {
        assert_eq!(every(8, 8), 1);
        assert_eq!(every(9, 8), 2);
        assert_eq!(every(256, 8), 50);
        assert_eq!(every(256, 16), 20);
        assert_eq!(every(0, 8), 1);
    }

    #[test]
    fn a_cell_is_named_by_its_labels_or_numbers() {
        let rows = vec!["sm0".to_string()];
        assert_eq!(cell_text(&rows, &[], (0, 3), Some(0.5)), "sm0, 3: 0.5");
        assert_eq!(cell_text(&[], &[], (2, 1), Some(f32::NAN)), "2, 1: no value");
    }
}

//! Metric charts (amui's `ui/charts.tsx`): an area trend as inline SVG, no
//! dependency, with the current value, the peak and the mean as text.
use aio::ui::prelude::*;

const W: f64 = 420.0;
const H: f64 = 90.0;
const P: f64 = 4.0;

/// The trend's paths (pure): the area, the line, the last point, the three
/// grid lines' heights, and the ceiling (15 % over the peak; at least 10
/// for a percentage, 1 otherwise).
pub struct Trend {
    pub area: String,
    pub line: String,
    pub last: Option<(f64, f64)>,
    pub grid: [f64; 3],
}

pub fn trend(values: &[f64], unit: &str) -> Trend {
    let n = values.len();
    let peak = values.iter().cloned().fold(0.0, f64::max);
    let ceil = (peak * 1.15).max(if unit == "%" { 10.0 } else { 1.0 });
    let x = |i: usize| if n <= 1 { W - P } else { P + i as f64 / (n - 1) as f64 * (W - 2.0 * P) };
    let y = |v: f64| H - P - v / ceil * (H - 2.0 * P);
    let pts: Vec<String> = values.iter().enumerate().map(|(i, v)| format!("{:.1},{:.1}", x(i), y(*v))).collect();
    let line = if pts.is_empty() { String::new() } else { format!("M{}", pts.join(" L")) };
    let area = if pts.is_empty() {
        String::new()
    } else {
        format!("{line} L{:.1},{} L{:.1},{} Z", x(n - 1), H - P, x(0), H - P)
    };
    Trend { area, line, last: values.last().map(|v| (x(n - 1), y(*v))), grid: [0.25, 0.5, 0.75].map(|f| y(ceil * f)) }
}

/// Peak and mean of `values` (pure).
pub fn peak_mean(values: &[f64]) -> (f64, f64) {
    let peak = values.iter().cloned().fold(0.0, f64::max);
    (peak, values.iter().sum::<f64>() / values.len().max(1) as f64)
}

/// A filled trend of `values` (amui's `AreaChart`), in `color` (a CSS colour).
#[component]
pub fn AreaChart(title: String, values: Vec<f64>, unit: String, color: String) -> View {
    let t = trend(&values, &unit);
    let (peak, avg) = peak_mean(&values);
    let digits = if unit == "%" { 1 } else { 0 };
    let fmt = |v: f64| format!("{v:.digits$}");
    let n = values.len();
    rsx! {
        div { class: "chart", "data-t": "chart-{title}",
            div { style: "display:flex;justify-content:space-between;align-items:baseline",
                span { class: "label", "{title}" }
                span { class: "mono", style: "font-size:18px;font-weight:700;color:{color}",
                    match values.last() {
                        Some(v) => fmt(*v),
                        None => "—".into(),
                    }
                    span { class: "dim", style: "font-size:11px", " {unit}" }
                }
            }
            svg { view_box: "0 0 {W} {H}", preserve_aspect_ratio: "none", "aria-hidden": "true",
                g {
                    for (i , y) in t.grid.iter().enumerate() {
                        line { key: "{i}", x1: "{P}", y1: "{y:.1}", x2: "{W - P}", y2: "{y:.1}", style: "stroke:var(--border)", stroke_width: "0.5", stroke_dasharray: "2 3" }
                    }
                }
                if !t.area.is_empty() {
                    path { d: "{t.area}", style: "fill:{color}", fill_opacity: "0.16" }
                    path { d: "{t.line}", fill: "none", style: "stroke:{color}", stroke_width: "1.6", stroke_linejoin: "round" }
                }
                if let Some((cx, cy)) = t.last {
                    circle { cx: "{cx:.1}", cy: "{cy:.1}", r: "2.4", style: "fill:{color}" }
                }
            }
            div { class: "small dim", style: "font-size:10px",
                if n > 0 {
                    "peak {fmt(peak)} · avg {fmt(avg)} {unit} · {n} samples"
                } else {
                    "—"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// amui's `AreaChart renders an SVG trend with peak/avg`.
    #[test]
    fn a_trend_tops_under_its_ceiling_and_says_peak_and_mean() {
        let t = trend(&[0.0, 10.0, 5.0], "MB");
        assert_eq!(t.line, "M4.0,86.0 L210.0,14.7 L416.0,50.3");
        assert!(t.area.starts_with(&t.line) && t.area.ends_with("L416.0,86 L4.0,86 Z"), "{}", t.area);
        let (lx, ly) = t.last.unwrap();
        assert!(lx == 416.0 && (ly - 50.348).abs() < 0.01, "{ly}");
        let one = trend(&[3.0], "%");
        assert_eq!(one.line, "M416.0,61.4", "one sample sits at the right; a % chart is 10 high at least");
        assert!(trend(&[], "%").line.is_empty());
        assert_eq!(peak_mean(&[1.0, 3.0, 2.0]), (3.0, 2.0));
        assert_eq!(peak_mean(&[]), (0.0, 0.0));
    }
}

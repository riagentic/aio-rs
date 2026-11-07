//! The page's audits (aio's run-time accessibility and contrast checks):
//! `TestUi::a11y`/`contrast` in a test, and in a dev build the running
//! page, its findings said in the log once each.
#![cfg_attr(
    not(any(feature = "testing", feature = "dev")),
    expect(dead_code, reason = "read by TestUi and a dev build")
)]
use serde_json::Value;

/// Text a person can't read on its background, from what the page's
/// `contrast` reader (`SURFACE_JS`) gave: each element whose colour over its
/// backdrop is under WCAG AA (pure).
pub(crate) fn contrast(all: &Value) -> Vec<String> {
    let rgba = |v: &Value| -> Option<Rgba> {
        let c: Vec<f32> = v.as_array()?.iter().filter_map(|n| Some(n.as_f64()? as f32)).collect();
        Some([*c.first()?, *c.get(1)?, *c.get(2)?, *c.get(3)?])
    };
    let judged = all.as_array().into_iter().flatten().filter_map(|e| {
        let layers: Vec<Rgba> = e["layers"].as_array()?.iter().filter_map(rgba).collect();
        let bg = layers.into_iter().rev().fold([1.0, 1.0, 1.0, 1.0], |under, l| over(l, under));
        let fg = rgba(&e["fg"]).filter(|c| c[3] > 0.05)?;
        let need = needed(e["size"].as_f64()? as f32, e["weight"].as_f64()? as f32);
        let ratio = contrast_ratio(over(fg, bg), bg);
        let (text, place) = (cut(e["text"].as_str()?, 30), e["place"].as_str()?);
        (ratio < need).then(|| format!("{place} {text:?} {ratio:.2}:1, needs {need}:1"))
    });
    judged.collect()
}

/// The page's findings now, `None` when nothing changed since the last ask
/// (a MutationObserver's mark): the a11y rules', then the contrast ones'.
#[cfg(feature = "dev")]
async fn findings() -> Option<Vec<String>> {
    let js = format!(
        "if (!window.__aioAudit) {{ const a = window.__aioAudit = {{ dirty: true, read: ({}) }}; \
         new MutationObserver(() => {{ a.dirty = true }}).observe(document.body, {{ subtree: true, childList: true, attributes: true, characterData: true }}) }} \
         const a = window.__aioAudit; if (!a.dirty) return null; a.dirty = false; \
         return [a.read('a11y', {{}}).ok, a.read('contrast', {{}}).ok]",
        crate::ui::SURFACE_JS
    );
    let v: Value = crate::ui::prelude::eval(&js).join().await.ok()?;
    let a11y = v[0].as_array()?.iter().filter_map(Value::as_str).map(|f| format!("accessibility: {f}"));
    Some(a11y.chain(contrast(&v[1]).into_iter().map(|f| format!("text a person can't read: {f}"))).collect())
}

/// A dev build's page, audited a second after it changes, as `TestUi`
/// audits a test's (aio's dev mode): each finding said once in the log
/// (`ui` warnings: the app's log on the desktop, `ax dev`'s under
/// `--web`). Started with the views.
#[cfg(feature = "dev")]
pub fn watch_page() {
    use std::collections::BTreeSet;
    use std::sync::Mutex;
    static SAID: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    crate::ui::prelude::spawn(async {
        loop {
            crate::ui::clock::sleep(std::time::Duration::from_secs(1)).await;
            for f in findings().await.unwrap_or_default() {
                if SAID.lock().unwrap_or_else(|e| e.into_inner()).insert(f.clone()) {
                    crate::warn!(cat: "ui", "{f}");
                }
            }
        }
    });
}

/// `s` cut to `max` chars, with `…`.
fn cut(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.into(),
    }
}

/// An sRGB colour, 0..=1 each, with alpha.
type Rgba = [f32; 4];

/// `fg` drawn over the opaque `bg`.
fn over(fg: Rgba, bg: Rgba) -> Rgba {
    let a = fg[3];
    [fg[0] * a + bg[0] * (1.0 - a), fg[1] * a + bg[1] * (1.0 - a), fg[2] * a + bg[2] * (1.0 - a), 1.0]
}

/// WCAG's contrast ratio of two opaque colours (1 to 21).
fn contrast_ratio(a: Rgba, b: Rgba) -> f32 {
    let lum = |c: Rgba| {
        let f = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// The ratio WCAG AA asks of text this size (px) and weight: 3 for large
/// (24 px, or 18.66 px bold), else 4.5.
fn needed(size: f32, weight: f32) -> f32 {
    if size >= 24.0 || (size >= 18.66 && weight >= 700.0) { 3.0 } else { 4.5 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_is_wcags_ratio_and_large_text_needs_less() {
        let (black, white) = ([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]);
        assert!((contrast_ratio(black, white) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(white, white) - 1.0).abs() < 0.01);
        let grey = 0x77 as f32 / 255.0;
        assert!((contrast_ratio([grey, grey, grey, 1.0], white) - 4.48).abs() < 0.01, "#777 just fails AA");
        assert_eq!(over([0.0, 0.0, 0.0, 0.5], white), [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(
            (needed(16.0, 400.0), needed(24.0, 400.0), needed(19.0, 700.0), needed(19.0, 400.0)),
            (4.5, 3.0, 3.0, 4.5)
        );
    }
}

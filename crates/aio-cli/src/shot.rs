//! `ax shot <app>` (aio's `am shot`): a PNG of the app's live window, over its
//! DevTools protocol (its own door; `--cdp <port>`: a port it opened, as
//! `ax start|dev <app> --cdp` or `AIORT_CDP=<port>`). Taken once the window painted
//! what it holds now. `--check base.png` asserts it still looks like a
//! baseline (exit 1 when not, with how it differs); `--update base.png`
//! records one. A channel may move by `--threshold` (0-255, default 2:
//! antialiasing) and a share `--max-diff` (0-1, default 0) of the pixels
//! may differ.
use aio::serde_json::json;
use image::RgbaImage;
use std::path::Path;

/// How far two shots may differ and still be the same.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerance {
    pub threshold: u8,
    pub max_ratio: f64,
}

/// The tolerance `--threshold` and `--max-diff` ask for (pure). A value
/// that can't be read is refused: a check that quietly ran with another
/// tolerance would pass what changed.
pub fn tolerance(threshold: Option<&str>, max_diff: Option<&str>) -> Result<Tolerance, String> {
    let threshold = match threshold {
        Some(t) => t.parse().map_err(|_| format!("--threshold {t}: a channel change of 0 to 255"))?,
        None => 2,
    };
    let max_ratio = match max_diff {
        Some(m) => m
            .parse()
            .ok()
            .filter(|r: &f64| (0.0..=1.0).contains(r))
            .ok_or(format!("--max-diff {m}: a share of 0 to 1"))?,
        None => 0.0,
    };
    Ok(Tolerance { threshold, max_ratio })
}

/// How `now` differs from `base` beyond `tol` (pure); `None`: the same.
/// A size change is said on its own: every pixel "differing" would send the
/// reader looking for a change that isn't there.
pub fn differs(now: &RgbaImage, base: &RgbaImage, tol: Tolerance) -> Option<String> {
    if now.dimensions() != base.dimensions() {
        let ((w, h), (bw, bh)) = (now.dimensions(), base.dimensions());
        return Some(format!("size changed: {bw}x{bh} baseline, {w}x{h} now"));
    }
    let worst = |(a, b): (&image::Rgba<u8>, &image::Rgba<u8>)| a.0.iter().zip(b.0).map(|(x, y)| x.abs_diff(y)).max();
    let deltas: Vec<u8> = now.pixels().zip(base.pixels()).filter_map(worst).collect();
    let n = deltas.iter().filter(|d| **d > tol.threshold).count();
    let ratio = if deltas.is_empty() { 0.0 } else { n as f64 / deltas.len() as f64 };
    (ratio > tol.max_ratio).then(|| {
        let max = deltas.iter().max().copied().unwrap_or(0);
        format!("{n} of {} pixels differ ({:.2}%, largest channel change {max})", deltas.len(), ratio * 100.0)
    })
}

fn decode(png: &[u8], what: &str) -> Result<RgbaImage, String> {
    image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map(|i| i.to_rgba8())
        .map_err(|e| format!("{what}: not a PNG ({e})"))
}

/// `--size WxH`: CSS px, each 1 to 16384.
pub fn size(s: &str) -> Result<(u32, u32), String> {
    let n = |v: &str| v.parse().ok().filter(|n| (1..=16384).contains(n));
    let (w, h) = s.split_once('x').ok_or_else(|| format!("--size {s}: WxH, such as 390x844"))?;
    n(w).zip(n(h)).ok_or_else(|| format!("--size {s}: WxH, each 1 to 16384 px"))
}

/// What to do with a shot besides keeping it.
pub enum Baseline<'a> {
    None,
    /// Assert it looks like this file.
    Check(&'a Path),
    /// Record it as this file.
    Update(&'a Path),
}

/// The app's window as a PNG, once it painted (two animation frames: the
/// second runs after a frame was committed); `selector`: one element of it,
/// as the page measures it; `full`: the whole page, past the viewport;
/// `size`: laid out at that size for the shot, then as it was.
async fn capture(
    (at, window): (&crate::cdp::Reach, usize),
    selector: Option<&str>,
    full: bool,
    size: Option<(u32, u32)>,
) -> Result<Vec<u8>, String> {
    let mut s = crate::cdp::window(at, window).await?;
    let Some((width, height)) = size else { return frame(&mut s, selector, full).await };
    let metrics = json!({ "width": width, "height": height, "deviceScaleFactor": 0, "mobile": false });
    s.call("Emulation.setDeviceMetricsOverride", metrics).await?;
    let png = frame(&mut s, selector, full).await;
    s.call("Emulation.clearDeviceMetricsOverride", json!({})).await?;
    png
}

/// The shot of the window `s` drives, once it painted.
async fn frame(s: &mut crate::cdp::Session, selector: Option<&str>, full: bool) -> Result<Vec<u8>, String> {
    s.eval(crate::cdp::PAINTED).await?;
    let mut params = json!({ "format": "png", "captureBeyondViewport": full });
    if let Some(sel) = selector {
        let q = aio::serde_json::to_string(sel).map_err(|e| e.to_string())?;
        let rect = s
            .eval(&format!(
                "(() => {{ const e = document.querySelector({q}); if (!e) return null; \
                 const r = e.getBoundingClientRect(); return {{ x: r.x, y: r.y, width: r.width, height: r.height }}; }})()"
            ))
            .await?;
        if rect.is_null() {
            return Err(format!("no element matches {sel} in the window: nothing to capture"));
        }
        if rect["width"].as_f64() == Some(0.0) || rect["height"].as_f64() == Some(0.0) {
            return Err(format!("{sel} has no box (display: none, or not laid out yet): nothing to capture"));
        }
        params["clip"] =
            json!({ "x": rect["x"], "y": rect["y"], "width": rect["width"], "height": rect["height"], "scale": 1 });
    }
    let r = s.call("Page.captureScreenshot", params).await?;
    let data = r["data"].as_str().ok_or("Page.captureScreenshot returned no data")?;
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| format!("Page.captureScreenshot: {e}"))
}

/// Take the shot into `out`, then check or record it.
pub fn run(
    out: &Path,
    at: (&crate::cdp::Reach, usize),
    (selector, full, size): (Option<&str>, bool, Option<(u32, u32)>),
    base: Baseline,
    tol: Tolerance,
) -> Result<String, String> {
    let png = aio::rt().block_on(capture(at, selector, full, size))?;
    let write = |p: &Path| std::fs::write(p, &png).map_err(|e| format!("{}: {e}", p.display()));
    match base {
        Baseline::Update(b) => {
            write(b)?;
            Ok(format!("recorded the baseline {}", b.display()))
        }
        Baseline::Check(b) => {
            let want =
                std::fs::read(b).map_err(|e| format!("{}: {e} (record it: --update {})", b.display(), b.display()))?;
            match differs(&decode(&png, "the shot")?, &decode(&want, &b.display().to_string())?, tol) {
                None => Ok(format!("matches {}", b.display())),
                Some(why) => {
                    write(out)?;
                    Err(format!("does not match {}: {why}; this shot is {}", b.display(), out.display()))
                }
            }
        }
        Baseline::None => {
            write(out)?;
            Ok(format!("wrote {}", out.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32, px: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba(px))
    }

    #[test]
    fn a_shot_differs_by_more_than_its_tolerance_or_by_size() {
        let base = img(10, 10, [100, 100, 100, 255]);
        let t = tolerance(None, None).unwrap();
        assert_eq!(t, Tolerance { threshold: 2, max_ratio: 0.0 });
        assert_eq!(differs(&base, &base, t), None);
        assert_eq!(differs(&img(10, 10, [102, 99, 100, 255]), &base, t), None, "antialiasing is the same");
        let mut moved = base.clone();
        moved.put_pixel(3, 3, image::Rgba([0, 100, 100, 255]));
        assert_eq!(differs(&moved, &base, t).unwrap(), "1 of 100 pixels differ (1.00%, largest channel change 100)");
        assert_eq!(differs(&moved, &base, tolerance(None, Some("0.01")).unwrap()), None, "within its share");
        assert_eq!(differs(&moved, &base, tolerance(Some("100"), None).unwrap()), None, "within its threshold");
        assert_eq!(differs(&img(12, 10, [0; 4]), &base, t).unwrap(), "size changed: 10x10 baseline, 12x10 now");
        assert!(tolerance(Some("abc"), None).unwrap_err().contains("0 to 255"));
        assert!(tolerance(Some("300"), None).is_err());
        assert!(tolerance(None, Some("2")).unwrap_err().contains("0 to 1"));
        assert!(tolerance(None, Some("NaN")).is_err(), "never a check that fails open");
        assert_eq!(size("390x844"), Ok((390, 844)));
        assert!(size("390").unwrap_err().contains("WxH") && size("0x10").is_err() && size("10x99999").is_err());
    }

    #[test]
    fn a_shot_is_taken_checked_and_recorded_over_the_protocol() {
        let mut png = vec![];
        img(4, 4, [9, 9, 9, 255]).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(&png);
        let answers = crate::cdp::tests::Answers::from([
            ("Page.captureScreenshot".into(), json!({ "data": data })),
            ("Runtime.evaluate".into(), json!({ "result": { "value": true } })),
            ("Emulation.setDeviceMetricsOverride".into(), json!({})),
            ("Emulation.clearDeviceMetricsOverride".into(), json!({})),
        ]);
        let port = &crate::cdp::Reach::Port(aio::rt().block_on(crate::cdp::tests::endpoint(answers)));
        let d = tempfile::tempdir().unwrap();
        let (out, base) = (d.path().join("now.png"), d.path().join("base.png"));
        let t = tolerance(None, None).unwrap();
        assert!(run(&out, (port, 0), (None, false, None), Baseline::None, t).unwrap().starts_with("wrote"));
        assert_eq!(std::fs::read(&out).unwrap(), png);
        assert!(run(&out, (port, 0), (None, false, None), Baseline::Check(&base), t).unwrap_err().contains("--update"));
        assert!(run(&out, (port, 0), (None, false, None), Baseline::Update(&base), t).unwrap().starts_with("recorded"));
        let e = run(&out, (port, 1), (None, false, None), Baseline::None, t).unwrap_err();
        assert!(e.contains("no window 1: the app has 1"), "a window that is not there: {e}");
        assert!(run(&out, (port, 0), (None, false, None), Baseline::Check(&base), t).unwrap().starts_with("matches"));
        assert!(run(&out, (port, 0), (None, false, Some((390, 844))), Baseline::None, t).unwrap().starts_with("wrote"));
        let mut other = vec![];
        img(4, 4, [200, 9, 9, 255]).write_to(&mut std::io::Cursor::new(&mut other), image::ImageFormat::Png).unwrap();
        std::fs::write(&base, other).unwrap();
        let e = run(&out, (port, 0), (None, false, None), Baseline::Check(&base), t).unwrap_err();
        assert!(e.contains("16 of 16 pixels differ") && e.contains("now.png"), "{e}");
    }
}

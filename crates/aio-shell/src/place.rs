//! Where the window opens, and what is saved of it (aio's `splash-place.ts`):
//! pure, so the rules are tested here; the runtime only reports screens and
//! bounds and applies the answer. Electron's coordinates: logical pixels,
//! origin at the primary display's top-left; screens are work areas.
use aio::serde_json::{self, Value, json};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// What `window.json` keeps: the normal rect (also while maximized or
/// fullscreen), those flags, and the size the app declared when it was saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    #[serde(flatten)]
    pub normal: Rect,
    pub maximized: bool,
    pub fullscreen: bool,
    pub declared_width: u32,
    pub declared_height: u32,
}

/// The window to open: its rect, and whether it opens maximized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub rect: Rect,
    pub maximized: bool,
}

/// How far inside its work area a window stays. A window manager draws
/// its frame outside the asked rect (Cinnamon: 10 px border, 37 px title); an
/// NVIDIA X driver faults on an off-screen frame (Xid 69) and can take Xorg
/// down. 64 covers the largest measured overhang (45) with room to spare.
pub const EDGE_MARGIN: i32 = 64;

/// The smallest saved size kept: anything less (a 1×1 window, saved while
/// the display was broken) would open every time as a window with nothing
/// to see, so the declared size is used instead.
pub const MIN_SAVED: (i32, i32) = (200, 150);

/// The window for declared size `dw`×`dh`, the `saved` state (if any) and
/// the `screens` (`primary` first when there are none). A saved size is kept
/// while the declared size is unchanged and at least [`MIN_SAVED`]; a saved position inside a screen
/// is kept, else it moves onto the screen it overlaps most; none: centred on
/// the primary. The rect stays [`EDGE_MARGIN`] inside its work area (less on
/// a small screen); one as big as that opens maximized.
pub fn place(dw: u32, dh: u32, saved: Option<&Saved>, screens: &[Rect], primary: Rect) -> Placed {
    let dw = i32::try_from(dw).unwrap_or(800).max(1);
    let dh = i32::try_from(dh).unwrap_or(600).max(1);
    let saved = saved.filter(|s| s.normal.width >= MIN_SAVED.0 && s.normal.height >= MIN_SAVED.1);
    let same = saved.is_some_and(|s| s.declared_width as i32 == dw && s.declared_height as i32 == dh);
    let (w0, h0) = match saved {
        Some(s) if same => (s.normal.width, s.normal.height),
        _ => (dw, dh),
    };
    let at = saved.map(|s| (s.normal.x, s.normal.y));
    let all = if screens.is_empty() { std::slice::from_ref(&primary) } else { screens };
    let s = match at {
        None => primary,
        Some((x, y)) => *all
            .iter()
            .find(|k| x >= k.x && y >= k.y && x + w0 <= k.x + k.width && y + h0 <= k.y + k.height)
            .unwrap_or_else(|| {
                let overlap = |k: &&Rect| {
                    let ix = ((x + w0).min(k.x + k.width) - x.max(k.x)).max(0) as i64;
                    let iy = ((y + h0).min(k.y + k.height) - y.max(k.y)).max(0) as i64;
                    ix * iy
                };
                // The first of equals: `max_by_key` keeps the last.
                all.iter().rev().max_by_key(overlap).unwrap_or(&primary)
            }),
    };
    let (mx, my) = (EDGE_MARGIN.min(s.width / 8), EDGE_MARGIN.min(s.height / 8));
    let (iw, ih) = ((s.width - 2 * mx).max(1), (s.height - 2 * my).max(1));
    let (w, h) = (w0.min(iw), h0.min(ih));
    let (lx, ly) = (s.x + mx, s.y + my);
    let (x, y) = match at {
        Some((x, y)) => (x.clamp(lx, lx + iw - w), y.clamp(ly, ly + ih - h)),
        None => (lx + (iw - w) / 2, ly + (ih - h) / 2),
    };
    let fills = w0 >= iw && h0 >= ih;
    Placed {
        rect: Rect { x, y, width: w, height: h },
        maximized: fills || saved.is_some_and(|s| s.maximized || s.fullscreen),
    }
}

/// The runtime's report of its screens, answered with where to open.
pub fn answer(screens: &Value, dw: u32, dh: u32, saved: Option<&Saved>) -> Value {
    let rects = |v: &Value| serde_json::from_value::<Vec<Rect>>(v.clone()).unwrap_or_default();
    let all = rects(&screens["screens"]);
    let primary = serde_json::from_value(screens["primary"].clone()).ok().or(all.first().copied()).unwrap_or(Rect {
        x: 0,
        y: 0,
        width: 1280,
        height: 800,
    });
    let p = place(dw, dh, saved, &all, primary);
    json!({ "t": "place", "x": p.rect.x, "y": p.rect.y, "width": p.rect.width, "height": p.rect.height, "maximized": p.maximized })
}

/// Read `window.json`; unreadable or missing is none (as Electron's own).
pub fn load(file: &std::path::Path) -> Option<Saved> {
    serde_json::from_slice(&std::fs::read(file).ok()?).ok()
}

/// Save the runtime's report of the window (atomically).
pub fn save(file: &std::path::Path, report: &Value, dw: u32, dh: u32) -> std::io::Result<()> {
    let normal: Rect = serde_json::from_value(report["normal"].clone()).map_err(std::io::Error::other)?;
    let s = Saved {
        normal,
        maximized: report["maximized"] == true,
        fullscreen: report["fullscreen"] == true,
        declared_width: dw,
        declared_height: dh,
    };
    aio::paths::write_atomic(file, serde_json::to_string(&s)?.as_bytes(), false)
}

/// A PNG's size in pixels, from its header. `None`: not a PNG.
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" || &png[12..16] != b"IHDR" {
        return None;
    }
    let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let (w, h) = (be(&png[16..20]), be(&png[20..24]));
    (w > 0 && h > 0).then_some((w, h))
}

/// The size an icon is shown at: at most 256 px a side, aspect kept.
/// `None`: not a PNG.
pub fn icon_size(png: &[u8]) -> Option<(u32, u32)> {
    let (w, h) = png_size(png)?;
    let big = w.max(h);
    if big <= 256 {
        return Some((w, h));
    }
    Some(((w * 256 / big).max(1), (h * 256 / big).max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FHD: Rect = Rect { x: 0, y: 0, width: 1920, height: 1040 };
    const RIGHT: Rect = Rect { x: 1920, y: 0, width: 1280, height: 1024 };

    fn saved(x: i32, y: i32, w: i32, h: i32) -> Saved {
        Saved {
            normal: Rect { x, y, width: w, height: h },
            declared_width: 1180,
            declared_height: 780,
            ..Default::default()
        }
    }

    fn inside(r: Rect, s: Rect) -> bool {
        r.x >= s.x + 64
            && r.y >= s.y + 64
            && r.x + r.width <= s.x + s.width - 64
            && r.y + r.height <= s.y + s.height - 64
    }

    #[test]
    fn a_new_window_is_centred_on_the_primary() {
        let p = place(1180, 780, None, &[FHD, RIGHT], FHD);
        assert_eq!(p.rect, Rect { x: 370, y: 130, width: 1180, height: 780 });
        assert!(!p.maximized);
    }

    #[test]
    fn windows_stay_64_px_inside_and_move_to_the_screen_they_overlap_most() {
        // Saved at the very edge: pulled 64 px in (the Xid 69 frame).
        let p = place(1180, 780, Some(&saved(0, 0, 1180, 780)), &[FHD], FHD);
        assert_eq!((p.rect.x, p.rect.y), (64, 64));
        // Mostly on the right screen: placed there, inside it.
        let p = place(1180, 780, Some(&saved(1800, 100, 1180, 780)), &[FHD, RIGHT], FHD);
        assert!(inside(p.rect, RIGHT), "{p:?}");
        // The right screen is gone: back onto the one that's left.
        let p = place(1180, 780, Some(&saved(2200, 100, 1180, 780)), &[FHD], FHD);
        assert!(inside(p.rect, FHD), "{p:?}");
        // A tiny screen: the margin shrinks to an eighth, and the window fits.
        let tiny = Rect { x: 0, y: 0, width: 400, height: 300 };
        let p = place(1180, 780, None, &[tiny], tiny);
        assert_eq!(p.rect, Rect { x: 50, y: 37, width: 300, height: 226 });
    }

    #[test]
    fn maximized_and_changed_sizes_and_screen_sized_windows() {
        // Maximized or fullscreen: the normal rect comes back, maximized.
        let mut s = saved(200, 100, 900, 600);
        s.maximized = true;
        let p = place(1180, 780, Some(&s), &[FHD], FHD);
        assert_eq!((p.rect, p.maximized), (Rect { x: 200, y: 100, width: 900, height: 600 }, true));
        s.maximized = false;
        s.fullscreen = true;
        assert!(place(1180, 780, Some(&s), &[FHD], FHD).maximized);
        // The app now declares another size: that beats the saved one.
        let p = place(1000, 700, Some(&saved(200, 100, 900, 600)), &[FHD], FHD);
        assert_eq!((p.rect.width, p.rect.height), (1000, 700));
        // As big as the screen: opens maximized.
        assert!(place(3000, 2000, None, &[FHD], FHD).maximized);
    }

    #[test]
    fn a_saved_size_too_small_to_see_is_ignored() {
        // window.json saved 1×1: the declared size, centred, comes back.
        assert_eq!(place(720, 820, Some(&saved(64, 64, 1, 1)), &[FHD], FHD), place(720, 820, None, &[FHD], FHD));
        let mut s = saved(64, 64, 199, 600);
        s.declared_width = 720;
        s.declared_height = 820;
        assert_eq!(place(720, 820, Some(&s), &[FHD], FHD).rect.width, 720);
        s.normal.width = 200;
        s.normal.height = 150;
        assert_eq!(place(720, 820, Some(&s), &[FHD], FHD).rect, Rect { x: 64, y: 64, width: 200, height: 150 });
    }

    #[test]
    fn the_state_saves_atomically_and_a_bad_one_is_none() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("window.json");
        let report = json!({ "normal": { "x": 10, "y": 20, "width": 900, "height": 600 }, "maximized": true });
        save(&f, &report, 1180, 780).unwrap();
        let s = load(&f).unwrap();
        assert_eq!((s.normal.width, s.maximized, s.fullscreen, s.declared_width), (900, true, false, 1180));
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1, "no temp file left");
        std::fs::write(&f, "{not json").unwrap();
        assert_eq!(load(&f), None);
        // The runtime's question, answered.
        let a = answer(&json!({ "screens": [FHD], "primary": FHD }), 1180, 780, None);
        assert_eq!(
            (a["t"].as_str(), a["x"].as_i64(), a["maximized"].as_bool()),
            (Some("place"), Some(370), Some(false))
        );
    }

    #[test]
    fn icons_are_shown_at_most_256_px() {
        let png = |w: u32, h: u32| {
            let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
            b.extend(w.to_be_bytes());
            b.extend(h.to_be_bytes());
            b
        };
        assert_eq!(icon_size(&png(128, 128)), Some((128, 128)));
        assert_eq!(icon_size(&png(1024, 1024)), Some((256, 256)));
        assert_eq!(icon_size(&png(1024, 512)), Some((256, 128)));
        assert_eq!(icon_size(b"GIF89a not a png at all"), None);
    }
}

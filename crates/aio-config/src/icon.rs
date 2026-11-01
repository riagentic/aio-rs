//! The icon of an app that has none (aio's `app-icon.ts`): its initial, drawn
//! from a stroked alphabet in code (no font: the same shape on every host),
//! deep on a pastel gradient in the app's hue ([`hue`], the theme's), so icon,
//! page and splash are one colour. A taskbar of three such apps shows three
//! marks, not three blue squares. Pure; as RGBA, SVG, and with `png` a PNG.
use std::f64::consts::PI;

/// `s` as a file name (aio's `slugify`): lowercase ASCII letters and digits,
/// each other run one `-`, none at the ends; empty: `aio-app`. A name with a
/// character beyond ASCII gets a short hash of itself too, so two such names
/// never meet.
pub fn slug(s: &str) -> String {
    let mut base = String::new();
    for c in s.to_lowercase().chars() {
        match c {
            'a'..='z' | '0'..='9' => base.push(c),
            _ if !base.ends_with('-') => base.push('-'),
            _ => {}
        }
    }
    let base = base.strip_prefix('-').unwrap_or(&base);
    let base = base.strip_suffix('-').unwrap_or(base);
    let base = if base.is_empty() { "aio-app" } else { base };
    match s.is_ascii() {
        true => base.into(),
        false => format!("{base}-{}", base36(fnv(s))),
    }
}

/// FNV-1a over `s`'s UTF-16 units (aio hashes JavaScript strings).
fn fnv(s: &str) -> u32 {
    s.encode_utf16().fold(0x811c_9dc5_u32, |h, c| (h ^ u32::from(c)).wrapping_mul(0x0100_0193))
}

fn base36(mut n: u32) -> String {
    let mut d = vec![];
    loop {
        d.push(char::from_digit(n % 36, 36).unwrap_or('0'));
        n /= 36;
        if n == 0 {
            break d.into_iter().rev().collect();
        }
    }
}

/// The app's hue from its id or its name (aio's `appHue`): FNV-1a over the
/// [`slug`] it makes, so `My Notes`, `my_notes` and `my-notes` are one
/// colour. `[ui] hue` overrides it: `Config::hue`.
pub fn hue(name: &str) -> u16 {
    (fnv(&slug(if name.is_empty() { "app" } else { name })) % 360) as u16
}

/// Pen width, in the 0..100 cap-height glyph space.
const STROKE: f64 = 11.0;
/// The cap height's share of the icon's edge.
const CAP_RATIO: f64 = 0.46;
/// The corner radius's share of the icon's edge.
const RADIUS_RATIO: f64 = 0.22;

/// Each glyph as subpaths (`|`): `M x y` move, `L x y` line, `B c1x c1y c2x
/// c2y x y` cubic, `C cx cy rx ry a0 a1` arc (degrees, its own subpath). y
/// grows down from the cap (0) to the baseline (100); centred when placed.
const GLYPHS: [(char, &str); 36] = [
    ('A', "M 0 100 L 38 0 L 76 100 | M 11 70 L 65 70"),
    ('B', "M 0 0 L 0 100 | C 0 25 46 25 -90 90 | C 0 75 52 25 -90 90"),
    ('C', "C 40 50 40 50 40 320"),
    ('D', "M 0 0 L 0 100 | C 0 50 66 50 -90 90"),
    ('E', "M 62 0 L 0 0 L 0 100 L 62 100 | M 0 50 L 50 50"),
    ('F', "M 62 0 L 0 0 L 0 100 | M 0 50 L 50 50"),
    ('G', "C 40 50 40 50 30 320 | M 74.6 75 L 74.6 50 L 48 50"),
    ('H', "M 0 0 L 0 100 | M 64 0 L 64 100 | M 0 50 L 64 50"),
    ('I', "M 0 0 L 0 100"),
    ('J', "M 60 0 L 60 72 | C 30 72 30 28 0 180"),
    ('K', "M 0 0 L 0 100 | M 60 0 L 4 52 | M 22 36 L 62 100"),
    ('L', "M 0 0 L 0 100 L 56 100"),
    ('M', "M 0 100 L 0 0 L 38 55 L 76 0 L 76 100"),
    ('N', "M 0 100 L 0 0 L 66 100 L 66 0"),
    ('O', "C 40 50 40 50 0 360"),
    ('P', "M 0 0 L 0 100 | C 0 28 48 28 -90 90"),
    ('Q', "C 40 50 40 50 0 360 | M 50 68 L 82 104"),
    ('R', "M 0 0 L 0 100 | C 0 28 48 28 -90 90 | M 22 56 L 66 100"),
    ('S', "M 62 16 B 58 -2 4 -2 4 28 B 4 50 62 48 62 72 B 62 100 8 102 2 84"),
    ('T', "M 0 0 L 64 0 | M 32 0 L 32 100"),
    ('U', "M 0 0 L 0 62 | C 32 62 32 38 0 180 | M 64 62 L 64 0"),
    ('V', "M 0 0 L 34 100 L 68 0"),
    ('W', "M 0 0 L 22 100 L 46 26 L 70 100 L 92 0"),
    ('X', "M 0 0 L 64 100 | M 64 0 L 0 100"),
    ('Y', "M 0 0 L 32 52 L 64 0 | M 32 52 L 32 100"),
    ('Z', "M 0 0 L 64 0 L 0 100 L 64 100"),
    ('0', "C 32 50 32 50 0 360"),
    ('1', "M 0 22 L 24 0 L 24 100"),
    ('2', "M 2 24 B 2 -4 60 -6 60 26 B 60 52 24 68 0 100 L 62 100"),
    ('3', "M 4 18 B 8 -4 58 -6 58 24 B 58 44 38 52 24 52 B 44 52 62 60 62 78 B 62 106 8 106 2 84"),
    ('4', "M 46 100 L 46 0 L 0 70 L 62 70"),
    ('5', "M 58 0 L 10 0 L 4 44 B 30 32 62 44 62 70 B 62 96 26 108 2 90"),
    ('6', "M 56 4 B 20 6 6 34 6 66 B 6 88 22 100 34 100 B 52 100 62 86 62 70 B 62 54 48 44 34 44 B 18 44 6 56 6 66"),
    ('7', "M 0 0 L 62 0 L 22 100"),
    ('8', "C 30 26 26 26 0 360 | C 30 74 32 26 0 360"),
    ('9', "M 8 96 B 44 94 58 66 58 34 B 58 12 42 0 30 0 B 12 0 2 14 2 30 B 2 46 16 56 30 56 B 46 56 58 44 58 34"),
];

/// For a name with no glyph-able character: a diamond, not a ring (a ring
/// reads as O, and a wrong letter is worse than none).
const FALLBACK: &str = "M 34 10 L 64 50 L 34 90 L 4 50 L 34 10";

type Pt = (f64, f64);

fn polylines(src: &str) -> Vec<Vec<Pt>> {
    let mut out = vec![];
    for sub in src.split('|') {
        let t: Vec<f64> = sub.split_whitespace().map(|w| w.parse().unwrap_or(f64::NAN)).collect();
        let cmds: Vec<&str> = sub.split_whitespace().collect();
        let (mut i, mut cur): (usize, Vec<Pt>) = (0, vec![]);
        while i < cmds.len() {
            let n = |k: usize| t[i + k];
            match cmds[i] {
                "M" => {
                    out.extend((cur.len() > 1).then(|| std::mem::take(&mut cur)));
                    cur = vec![(n(1), n(2))];
                    i += 3;
                }
                "L" => {
                    cur.push((n(1), n(2)));
                    i += 3;
                }
                "B" => {
                    let (x0, y0) = cur.last().copied().unwrap_or((0.0, 0.0));
                    let (c1, c2, p) = ((n(1), n(2)), (n(3), n(4)), (n(5), n(6)));
                    cur.extend((1..=16).map(|k| {
                        let (u, v) = (f64::from(k) / 16.0, 1.0 - f64::from(k) / 16.0);
                        let at = |a: f64, b: f64, c: f64, d: f64| {
                            v * v * v * a + 3.0 * v * v * u * b + 3.0 * v * u * u * c + u * u * u * d
                        };
                        (at(x0, c1.0, c2.0, p.0), at(y0, c1.1, c2.1, p.1))
                    }));
                    i += 7;
                }
                _ => {
                    out.extend((cur.len() > 1).then(|| std::mem::take(&mut cur)));
                    let (cx, cy, rx, ry, a0, a1) = (n(1), n(2), n(3), n(4), n(5), n(6));
                    let steps = ((a1 - a0).abs() / 6.0).round().max(8.0) as u32;
                    cur = (0..=steps)
                        .map(|k| {
                            let a = (a0 + (a1 - a0) * f64::from(k) / f64::from(steps)) * PI / 180.0;
                            (cx + rx * a.cos(), cy + ry * a.sin())
                        })
                        .collect();
                    i += 7;
                }
            }
        }
        out.extend((cur.len() > 1).then_some(cur));
    }
    out
}

/// The character a name is drawn with: its first one with a glyph, accents
/// folded (Ü is U, not the next letter); else its first, uppercased.
pub fn monogram(name: &str) -> char {
    let fold = |c: char| match c {
        'À'..='Å' | 'à'..='å' | 'Ā'..='ą' => 'A',
        'Ç' | 'ç' | 'Ć'..='č' => 'C',
        'Ď'..='đ' => 'D',
        'È'..='Ë' | 'è'..='ë' | 'Ē'..='ě' => 'E',
        'Ĝ'..='ģ' => 'G',
        'Ì'..='Ï' | 'ì'..='ï' | 'Ĩ'..='ı' => 'I',
        'Ñ' | 'ñ' | 'Ń'..='ň' => 'N',
        'Ò'..='Ö' | 'Ø' | 'ò'..='ö' | 'ø' | 'Ō'..='ő' => 'O',
        'Ŕ'..='ř' => 'R',
        'ß' | 'Ś'..='š' => 'S',
        'Ţ'..='ť' => 'T',
        'Ù'..='Ü' | 'ù'..='ü' | 'Ũ'..='ų' => 'U',
        'Ý' | 'ý' | 'ÿ' => 'Y',
        'Ź'..='ž' => 'Z',
        c => c.to_ascii_uppercase(),
    };
    let name = name.trim();
    name.chars()
        .map(fold)
        .find(|c| GLYPHS.iter().any(|(g, _)| g == c))
        .or_else(|| name.chars().next().map(|c| c.to_uppercase().next().unwrap_or(c)))
        .unwrap_or('?')
}

/// HSL (h in degrees, s and l in 0..1) as 8-bit sRGB.
fn rgb(h: f64, s: f64, l: f64) -> [f64; 3] {
    let a = s * l.min(1.0 - l);
    [0.0, 8.0, 4.0].map(|n| {
        let k = (n + h / 30.0) % 12.0;
        ((l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)) * 255.0).round()
    })
}

fn luminance(c: [f64; 3]) -> f64 {
    let [r, g, b] = c.map(|v| {
        let x = v / 255.0;
        if x <= 0.03928 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
    });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// The ink's lightness: the lightest that clears AA (4.6, over 8-bit
/// rounding) against the gradient's darker end.
fn ink_lightness(hue: f64) -> u32 {
    let bg = luminance(rgb((hue + 24.0) % 360.0, 0.62, 0.68)) + 0.05;
    (8..=30).rev().find(|&l| bg / (luminance(rgb(hue, 0.70, f64::from(l) / 100.0)) + 0.05) >= 4.6).unwrap_or(8)
}

/// The glyph of `name` placed in a `size` square: cap-height scaled, centred
/// on its own box; and the pen width.
fn placed(name: &str, size: f64) -> (Vec<Vec<Pt>>, f64) {
    let c = monogram(name);
    let raw = polylines(GLYPHS.iter().find(|(g, _)| *g == c).map_or(FALLBACK, |(_, s)| s));
    let scale = size * CAP_RATIO / 100.0;
    let all = raw.iter().flatten();
    let (x0, x1) = all.clone().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (y0, y1) = all.fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
    let (dx, dy) = (size / 2.0 - (x0 + x1) / 2.0 * scale, size / 2.0 - (y0 + y1) / 2.0 * scale);
    let pts = raw.into_iter().map(|l| l.into_iter().map(|(x, y)| (x * scale + dx, y * scale + dy)).collect()).collect();
    (pts, STROKE * scale)
}

fn seg_dist((px, py): Pt, (ax, ay): Pt, (bx, by): Pt) -> f64 {
    let (vx, vy, wx, wy) = (bx - ax, by - ay, px - ax, py - ay);
    let len = vx * vx + vy * vy;
    let t = if len == 0.0 { 0.0 } else { ((wx * vx + wy * vy) / len).clamp(0.0, 1.0) };
    (wx - t * vx).hypot(wy - t * vy)
}

/// Signed distance to the rounded box filling a `size` square.
fn box_dist((px, py): Pt, size: f64, r: f64) -> f64 {
    let q = |p: f64| (p - size / 2.0).abs() - (size / 2.0 - r);
    let (qx, qy) = (q(px), q(py));
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// 1 inside, 0 outside, antialiased across a pixel.
fn cover(d: f64) -> f64 {
    (0.5 - d).clamp(0.0, 1.0)
}

/// The ends of the gradient and the ink, 8-bit sRGB.
fn colors(hue: u16) -> ([f64; 3], [f64; 3], [f64; 3]) {
    let h = f64::from(hue % 360);
    (rgb(h, 0.68, 0.82), rgb((h + 24.0) % 360.0, 0.62, 0.68), rgb(h, 0.70, f64::from(ink_lightness(h)) / 100.0))
}

/// The icon of an app (`name` draws the letter) in `hue`, as RGBA.
pub fn pixels(name: &str, hue: u16, size: u32) -> Vec<u8> {
    let (c0, c1, ink) = colors(hue);
    let s = f64::from(size);
    let (pts, width) = placed(name, s);
    let r = s * RADIUS_RATIO;
    // Only the glyph's neighbourhood needs the distance loop.
    let pad = width / 2.0 + 2.0;
    let all = pts.iter().flatten();
    let (gx0, gx1) = all.clone().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (gy0, gy1) = all.fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
    let (xs, ys) = (gx0 - pad..=gx1 + pad, gy0 - pad..=gy1 + pad);
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        let t = f64::from(y) / f64::from(size.max(2) - 1);
        let bg: [f64; 3] = std::array::from_fn(|i| c0[i] + (c1[i] - c0[i]) * t);
        for x in 0..size {
            let p = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let box_a = cover(box_dist(p, s, r));
            let a = match box_a > 0.0 && xs.contains(&p.0) && ys.contains(&p.1) {
                true => {
                    let best = pts.iter().flat_map(|l| l.windows(2).map(|w| seg_dist(p, w[0], w[1])));
                    cover(best.fold(f64::MAX, f64::min) - width / 2.0)
                }
                false => 0.0,
            };
            px.extend((0..3).map(|i| (bg[i] + (ink[i] - bg[i]) * a).round() as u8));
            px.push((box_a * 255.0).round() as u8);
        }
    }
    px
}

/// The icon as an SVG document, font-free: the same mark in a browser tab,
/// a `.desktop` entry or a README as in the PNG.
pub fn svg(name: &str, hue: u16, size: u32) -> String {
    let h = hue % 360;
    let s = f64::from(size);
    let (pts, width) = placed(name, s);
    let d: Vec<String> = pts
        .iter()
        .map(|l| {
            let at = l.iter().enumerate().map(|(i, (x, y))| format!("{}{x:.2} {y:.2}", if i == 0 { 'M' } else { 'L' }));
            at.collect::<Vec<_>>().join(" ")
        })
        .collect();
    let (r, fg) = (s * RADIUS_RATIO, ink_lightness(f64::from(h)));
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 {size} {size}">
  <defs><linearGradient id="g{h}" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="hsl({h} 68% 82%)"/><stop offset="1" stop-color="hsl({} 62% 68%)"/>
  </linearGradient></defs>
  <rect width="{size}" height="{size}" rx="{r:.2}" fill="url(#g{h})"/>
  <path d="{}" fill="none" stroke="hsl({h} 70% {fg}%)" stroke-width="{width:.2}" stroke-linecap="round" stroke-linejoin="round"/>
</svg>
"#,
        (h + 24) % 360,
        d.join(" ")
    )
}

/// RGBA pixels, `w` by `h`, as a PNG.
#[cfg(feature = "png")]
pub fn png(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    use std::io::Write;
    let mut raw = Vec::with_capacity(rgba.len() + h as usize);
    for row in rgba.chunks(w as usize * 4) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }
    let mut z = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::best());
    _ = z.write_all(&raw);
    let idat = z.finish().unwrap_or_default();
    let chunk = |kind: &[u8; 4], data: &[u8]| {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        let mut crc = flate2::Crc::new();
        crc.update(&c[4..]);
        c.extend(crc.sum().to_be_bytes());
        c
    };
    let mut ihdr = [w.to_be_bytes(), h.to_be_bytes()].concat();
    ihdr.extend([8, 6, 0, 0, 0]); // 8-bit RGBA
    [b"\x89PNG\r\n\x1a\n".to_vec(), chunk(b"IHDR", &ihdr), chunk(b"IDAT", &idat), chunk(b"IEND", &[])].concat()
}

/// The app's icon as a PNG of `size`.
#[cfg(feature = "png")]
pub fn icon_png(name: &str, hue: u16, size: u32) -> Vec<u8> {
    png(&pixels(name, hue, size), size, size)
}

/// A PNG's width and height; `None`: not a PNG.
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let be = |i: usize| u32::from_be_bytes([png[i], png[i + 1], png[i + 2], png[i + 3]]);
    (png.len() >= 24 && png.starts_with(b"\x89PNG\r\n\x1a\n") && &png[12..16] == b"IHDR").then(|| (be(16), be(20)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slug_is_aios() {
        // aio tests/app-id.test.ts.
        assert_eq!(slug("My Notes"), "my-notes");
        assert_eq!(slug("my_notes"), "my-notes");
        assert_eq!(slug("--a..b--"), "a-b");
        assert_eq!(slug("!!!"), "aio-app");
        assert_eq!(slug(""), "aio-app");
        // Beyond ASCII: a hash of the original, so "Über" is not "Ber".
        assert_eq!(slug("Über"), format!("ber-{}", base36(fnv("Über"))));
        assert_ne!(slug("Über"), slug("Ber"));
        assert_eq!(base36(0), "0");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
    }

    #[test]
    fn one_identity_is_one_hue_however_it_is_spelled() {
        assert_eq!(hue("todo"), 61);
        assert_eq!(hue("My Notes"), hue("my-notes"));
        assert_eq!(hue("my_notes"), hue("my-notes"));
        assert_eq!(hue(""), hue("app"), "a nameless app still has a hue");
    }

    #[test]
    fn the_letter_is_the_names_first_with_a_glyph_accents_folded() {
        assert_eq!(monogram("notes"), 'N');
        assert_eq!(monogram("  my-app"), 'M');
        assert_eq!(monogram("3d-viewer"), '3');
        assert_eq!((monogram("Über"), monogram("Éclair"), monogram("ßtraße")), ('U', 'E', 'S'));
        assert_eq!(monogram("-_ x"), 'X');
        assert_eq!(monogram("заметки"), 'З', "no glyph: its first letter (drawn as the diamond)");
        assert_eq!(monogram(""), '?');
    }

    #[test]
    fn the_ink_is_readable_on_every_hue() {
        for h in 0..360 {
            let h = f64::from(h);
            let bg = luminance(rgb((h + 24.0) % 360.0, 0.62, 0.68)) + 0.05;
            let fg = luminance(rgb(h, 0.70, f64::from(ink_lightness(h)) / 100.0)) + 0.05;
            assert!(bg / fg >= 4.5, "hue {h}: {}", bg / fg);
        }
    }

    #[test]
    fn the_icon_is_a_rounded_tile_with_the_letter_in_the_middle() {
        let px = pixels("Notes", hue("notes"), 64);
        assert_eq!(px.len(), 64 * 64 * 4);
        let at = |x: usize, y: usize| &px[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
        assert_eq!(at(0, 0)[3], 0, "the corner is outside the rounded box");
        assert_eq!(at(32, 2)[3], 255, "the edge's middle is inside");
        let bg = rgb(f64::from(hue("notes")), 0.68, 0.82);
        assert!((0..3).all(|i| (f64::from(at(32, 1)[i]) - bg[i]).abs() < 2.0), "pastel at the top");
        // N's diagonal crosses the centre: ink there, not background.
        assert!(f64::from(at(32, 32)[0]) < bg[0] - 60.0, "{:?}", at(32, 32));
        assert_eq!(pixels("Notes", hue("notes"), 64), px, "the same every time");
        assert_ne!(pixels("Todo", hue("notes"), 64), px, "another letter");
        assert_ne!(pixels("Notes", 200, 64)[..4 * 64 * 2], px[..4 * 64 * 2], "another hue");
        let top = px[..4 * 64 * 4].chunks(4).all(|p| (f64::from(p[0]) - bg[0]).abs() < 4.0);
        assert!(top, "no ink near the edge");
    }

    #[test]
    fn the_svg_draws_the_same_mark_in_the_same_colours() {
        let s = svg("Notes", 210, 512);
        assert!(s.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512""#), "{s}");
        assert!(s.contains(r#"stop-color="hsl(210 68% 82%)""#) && s.contains(r#"stop-color="hsl(234 62% 68%)""#));
        assert!(s.contains(r#"rx="112.64""#) && s.contains(r#"fill="url(#g210)""#));
        let (pts, width) = placed("Notes", 512.0);
        assert!(s.contains(&format!(r#"stroke-width="{width:.2}""#)));
        let d = s.split(" d=\"").nth(1).unwrap();
        assert_eq!(d[..d.find('"').unwrap()].matches('M').count(), pts.len(), "one subpath per polyline");
        assert!(!s.contains("<text"), "no font");
    }

    /// What aio 1.0.27 makes of the same input (its `slugify`, `appHue`,
    /// `appIconSvg`, `appIconPixels`), run once and kept here.
    #[test]
    fn it_is_aios_icon_byte_for_byte() {
        let slugs = ["My Notes", "Über", "заметки", "!!!", "--a..b--"].map(slug);
        assert_eq!(slugs, ["my-notes", "ber-1mp5imw", "aio-app-uvmkg7", "aio-app", "a-b"]);
        assert_eq!(["todo", "My Notes", "Über", "", "notes", "kit"].map(hue), [61, 327, 231, 252, 226, 327]);
        assert_eq!(
            svg("Notes", 210, 512),
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"512\" height=\"512\" viewBox=\"0 0 512 512\">\n  \
             <defs><linearGradient id=\"g210\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\">\n    \
             <stop offset=\"0\" stop-color=\"hsl(210 68% 82%)\"/><stop offset=\"1\" stop-color=\"hsl(234 62% 68%)\"/>\n  \
             </linearGradient></defs>\n  <rect width=\"512\" height=\"512\" rx=\"112.64\" fill=\"url(#g210)\"/>\n  \
             <path d=\"M178.28 373.76 L178.28 138.24 L333.72 373.76 L333.72 138.24\" fill=\"none\" \
             stroke=\"hsl(210 70% 14%)\" stroke-width=\"25.91\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>\n</svg>\n"
        );
        let sum = |px: &[u8]| px.iter().fold(7u32, |a, &b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
        assert_eq!(sum(&pixels("Notes", 210, 64)), 3649856572);
        assert_eq!(sum(&pixels("8ball", 7, 48)), 2955637925);
    }

    #[cfg(feature = "png")]
    #[test]
    fn the_png_is_well_formed() {
        let p = icon_png("Notes", hue("notes"), 16);
        assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(png_size(&p), Some((16, 16)));
        assert_eq!((p[24], p[25]), (8, 6));
        let mut crc = flate2::Crc::new();
        crc.update(&p[12..29]);
        assert_eq!(&p[29..33], crc.sum().to_be_bytes());
        assert!(p.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82]));
        assert_eq!(png_size(b"GIF89a"), None);
    }
}

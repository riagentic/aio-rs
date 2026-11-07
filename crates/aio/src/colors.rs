//! The app's colours from one hue (aio's `app-theme.ts`): the kit's theme
//! tokens (`kit::theme`) and what is drawn outside the page, the splash and
//! the first-run download window ([`identity`]). Pure.

/// The app's hue from its id (aio's `appHue`: FNV-1a over the slug it
/// makes); an app's own is `Config::hue` (`[ui] hue` first).
pub use aio_config::icon::hue;

pub(crate) type Hsl = (f64, f64, f64);

/// The sRGB channels of an HSL colour (h in degrees, s and l in 0..1), before gamma.
pub(crate) fn rgb((h, s, l): Hsl) -> [f64; 3] {
    let a = s * l.min(1.0 - l);
    [0.0, 8.0, 4.0].map(|n| {
        let k = (n + h / 30.0) % 12.0;
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    })
}

/// Relative luminance (WCAG's gamma step, as aio has it).
pub(crate) fn luminance(c: Hsl) -> f64 {
    let [r, g, b] = rgb(c).map(|v| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// `#rrggbb`, for colours that need an alpha suffix.
pub(crate) fn hex(c: Hsl) -> String {
    rgb(c).iter().map(|v| format!("{:02x}", (255.0 * v).round() as u8)).fold("#".into(), |s, x| s + &x)
}

pub(crate) fn contrast(a: f64, b: f64) -> f64 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The accent fill and the ink on it: the vivid colour with dark ink when it
/// carries one, else a darker fill with white (yellows stay yellow).
/// (`hue` an integer: its text no float formatting, which a page then
/// leaves out: ~10 KB of wasm.)
pub(crate) fn accent_fill(hue: u16) -> (String, &'static str, Hsl) {
    let h = f64::from(hue);
    let ink = luminance((h, 0.45, 0.11));
    if let Some(l) = (50..=64).rev().find(|&l| contrast(luminance((h, 0.82, f64::from(l) / 100.0)), ink) >= 4.6) {
        return (format!("hsl({hue} 82% {l}%)"), "dark", (h, 0.82, f64::from(l) / 100.0));
    }
    let l = (20..=52).rev().find(|&l| contrast(luminance((h, 0.72, f64::from(l) / 100.0)), 1.0) >= 4.6).unwrap_or(20);
    (format!("hsl({hue} 72% {l}%)"), "#fff", (h, 0.72, f64::from(l) / 100.0))
}

/// The app's colours outside its page (the splash, the download window),
/// as the theme has them for `hue`: `[light, dark]`, each `[background,
/// text, bar, track]` as `#rrggbb` (the page's `--aio-bg`, `--aio-text`,
/// `--aio-accent` and `--aio-border`).
pub fn identity(hue: u16) -> [[String; 4]; 2] {
    let h = f64::from(hue);
    let (_, _, bar) = accent_fill(hue);
    [
        [hex((h, 0.4, 0.99)), hex((h, 0.14, 0.12)), hex(bar), hex((h, 0.12, 0.88))],
        [hex((h, 0.14, 0.09)), hex((h, 0.1, 0.94)), hex(bar), hex((h, 0.1, 0.26))],
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_identity_is_the_theme_s_own_colours_and_readable() {
        let [light, dark] = super::identity(210);
        assert_eq!(light[0], super::hex((210.0, 0.4, 0.99)));
        let lum = |c: &str| {
            let v = |i: usize| f64::from(u8::from_str_radix(&c[i..i + 2], 16).unwrap()) / 255.0;
            let g = |x: f64| if x <= 0.03928 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) };
            0.2126 * g(v(1)) + 0.7152 * g(v(3)) + 0.0722 * g(v(5))
        };
        for [bg, text, ..] in [&light, &dark] {
            assert!(super::contrast(lum(bg), lum(text)) >= 7.0, "{bg} {text}");
        }
        assert!(light.iter().chain(&dark).all(|c| c.len() == 7 && c.starts_with('#')));
    }
}

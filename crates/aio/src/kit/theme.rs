//! The default theme's tokens (aio's `app-theme.ts`): every `--aio-*` colour is
//! made from one hue, hashed from the app's id so a rename never recolours.
//! The two inks are solved for contrast against that hue (the fill against
//! its label, text against every background it lands on), so a theme is
//! readable whatever the hue. Pure; byte-for-byte aio's output.

pub use crate::colors::hue;
use crate::colors::{Hsl, accent_fill, contrast, hex, luminance, rgb};

/// The accent as text, readable (4.6:1) on every background in `bgs`.
fn accent_ink(hue: f64, bgs: &[f64], dark: bool) -> String {
    let mut range: Box<dyn Iterator<Item = u32>> = if dark { Box::new(48..93) } else { Box::new((8..=52).rev()) };
    match range.find(|&l| bgs.iter().all(|&bg| contrast(luminance((hue, 0.68, f64::from(l) / 100.0)), bg) >= 4.6)) {
        Some(l) => format!("hsl({hue} 68% {l}%)"),
        None if dark => format!("hsl({hue} 68% 92%)"),
        None => format!("hsl({hue} 68% 8%)"),
    }
}

/// Luminance of `color-mix(in oklab, a p, b)` (`--aio-tint`).
fn oklab_mix_luminance(a: Hsl, b: Hsl, p: f64) -> f64 {
    let lms = |c: Hsl| {
        let [r, g, b] = rgb(c).map(|v| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) });
        [
            (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt(),
            (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt(),
            (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt(),
        ]
    };
    let (x, y) = (lms(a), lms(b));
    let [l, m, s] = [0, 1, 2].map(|i| (x[i] * p + y[i] * (1.0 - p)).powi(3));
    let c = |v: f64| v.clamp(0.0, 1.0);
    0.2126 * c(4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s)
        + 0.7152 * c(-1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s)
        + 0.0722 * c(-0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701 * s)
}

/// A percentage to one decimal, as JS prints it (`14`, `8.5`).
fn pct(x: f64) -> String {
    let v = (x * 1000.0).round() / 10.0;
    if v.fract() == 0.0 { format!("{v:.0}%") } else { format!("{v}%") }
}

/// The theme's tokens for `hue` (aio's `appThemeTokensCss`), light and dark, in
/// `@layer aio` so any app rule wins. `soft` keeps the neutrals near grey
/// (aio's `"neutral": "soft"`); the accent keeps the hue.
pub fn tokens_css(hue: u16, soft: bool) -> String {
    let h = f64::from(hue);
    let ns = |s: f64| if soft { s.min(0.04) } else { s };
    let (accent, on_accent, fill) = accent_fill(h);
    let on_accent = if on_accent == "dark" { format!("hsl({hue} 45% 11%)") } else { on_accent.into() };
    let (surf_light, surf_dark) = ((0.0, 0.0, 1.0), (h, ns(0.12), 0.13));
    let ink_light = accent_ink(
        h,
        &[
            luminance((h, ns(0.4), 0.99)),
            luminance(surf_light),
            luminance((h, ns(0.3), 0.97)),
            oklab_mix_luminance(fill, surf_light, 0.1),
        ],
        false,
    );
    let ink_dark = accent_ink(
        h,
        &[
            luminance((h, ns(0.14), 0.09)),
            luminance(surf_dark),
            luminance((h, ns(0.1), 0.17)),
            oklab_mix_luminance(fill, surf_dark, 0.1),
        ],
        true,
    );
    let n = |l: u32, s: f64| format!("hsl({hue} {} {l}%)", pct(ns(s)));
    let shadow = hex((h, ns(0.14), 0.12));
    format!(
        r#"@layer aio {{
:root{{
  --aio-hue:{hue};
  --aio-accent:{accent};
  --aio-on-accent:{on_accent};
  --aio-accent-ink:{ink_light};
  --aio-bg:{bg};
  --aio-surface:#fff;
  --aio-surface-2:{s2};
  --aio-text:{text};
  --aio-muted:{muted};
  --aio-border:{border};
  --aio-danger:hsl(2 72% 45%);
  --aio-on-danger:#fff;
  --aio-ok:hsl(152 62% 32%);
  --aio-warn:hsl(38 82% 38%);
  --aio-r-1:6px; --aio-r-2:10px; --aio-r-3:14px; --aio-r-4:20px;
  --aio-page:72rem;
  --aio-s-1:.25rem; --aio-s-2:.5rem; --aio-s-3:.75rem;
  --aio-s-4:1rem; --aio-s-5:1.5rem; --aio-s-6:2.5rem;
  --aio-font:ui-sans-serif,system-ui,-apple-system,"Segoe UI",Inter,Roboto,
    "Helvetica Neue",Arial,sans-serif;
  --aio-mono:ui-monospace,SFMono-Regular,"SF Mono",Menlo,Consolas,
    "Liberation Mono",monospace;
  --aio-shadow-1:0 1px 2px {shadow}14;
  --aio-shadow-2:0 6px 24px -8px {shadow}2e,0 1px 2px {shadow}14;
  --aio-ring:0 0 0 3px color-mix(in oklab, var(--aio-accent) 35%, transparent);
  --aio-tint:color-mix(in oklab, var(--aio-accent) 10%, var(--aio-surface));
}}
@media (prefers-color-scheme: dark){{:root{{
  --aio-accent-ink:{ink_dark};
  --aio-bg:{bg_d};
  --aio-surface:{surf_d};
  --aio-surface-2:{s2_d};
  --aio-text:{text_d};
  --aio-muted:{muted_d};
  --aio-border:{border_d};
  --aio-danger:hsl(2 78% 66%);
  --aio-ok:hsl(152 52% 58%);
  --aio-warn:hsl(38 84% 62%);
  --aio-shadow-1:0 1px 2px #0006;
  --aio-shadow-2:0 8px 28px -10px #0009,0 1px 2px #0006;
}}}}
{DANGER_INK}
}}
"#,
        bg = n(99, 0.4),
        s2 = n(97, 0.3),
        text = n(12, 0.14),
        muted = n(42, 0.08),
        border = n(88, 0.12),
        bg_d = n(9, 0.14),
        surf_d = n(13, 0.12),
        s2_d = n(17, 0.1),
        text_d = n(94, 0.1),
        muted_d = n(64, 0.08),
        border_d = n(26, 0.1),
    )
}

/// The hue-free tail of the tokens: the danger ink solved from its fill, and
/// the high-contrast token values.
const DANGER_INK: &str = r#"/* The ink ON a danger fill, solved from the fill itself: white where the
   fill is darker than the WCAG white/black crossover (relative luminance
   1791 in ten-thousandths), black where it is lighter — the better of the two, never under
   4.58:1. A fixed dark ink for the lightened dark-mode fill broke every app
   that sets its own --aio-danger (white on #d32f2f became 3.65:1); an engine
   without relative colour keeps white, as before. */
@supports (color: color(from red srgb-linear calc(r * 0.5) g b)){:root{
  --aio-on-danger:color(from var(--aio-danger) srgb-linear calc(clamp(0, (1791 - (2126 * r + 7152 * g + 722 * b)) * 10, 1)) calc(clamp(0, (1791 - (2126 * r + 7152 * g + 722 * b)) * 10, 1)) calc(clamp(0, (1791 - (2126 * r + 7152 * g + 722 * b)) * 10, 1)));
}}

/* "prefers-contrast: more", the TOKEN half: inert custom properties, so it
   belongs with the tokens every mode keeps — the kit and any app reading
   var(--aio-border) get the high-contrast values in "tokens" mode too. The
   rules that paint (focus ring, border width) stay in the visual half. */
@media (prefers-contrast:more){:root{
  --aio-border:var(--aio-text);
  --aio-muted:var(--aio-text);
  --aio-shadow-1:none;
  --aio-shadow-2:none;
}}"#;

/// Where the kit's own controls start in [`super::CSS`]; the look is before.
const KIT_START: &str = ":root {\n  --aio-ui-accent:";

/// The look's layout regions (start and end banners; aio's `SHELL_REGIONS`):
/// the `<main>` page shell and the layout classes.
const LAYOUT: [(&str, &str); 2] =
    [("/* ── page shell ", "/* ── type "), ("/* ── the six classes worth having ", "/* ── the three environments ")];

/// The aio CSS a page takes before its own, from `[ui]` (aio's `ui.theme`,
/// `ui.hue`, `ui.layout`): the tokens, from `ui.hue` or the hue of app `id`,
/// with the kit's controls, and the look unless `ui.theme` leaves it out
/// (`Auto` does when the app `ships_css`). `Look::None`: nothing.
#[cfg(not(target_arch = "wasm32"))]
pub fn sheet(ui: &crate::config::Ui, id: &str, ships_css: bool) -> String {
    use crate::config::{Look, Neutral};
    let css = super::CSS;
    let kit = css.find(KIT_START).expect("kit.css: the kit's controls start with the --aio-ui-* tokens");
    let look = match ui.theme {
        Look::None => return String::new(),
        Look::Tokens => false,
        Look::Auto => !ships_css,
        Look::Full => true,
    };
    let mut out = tokens_css(ui.hue.unwrap_or_else(|| hue(id)), ui.neutral == Neutral::Soft) + "\n";
    if look {
        let mut part = &css[..kit];
        let mut kept = String::new();
        for (start, end) in LAYOUT.iter().filter(|_| !ui.layout) {
            let a = part.find(start).expect("kit.css: a layout banner");
            let b = a + part[a..].find(end).expect("kit.css: the banner after a layout region");
            kept.push_str(&part[..a]);
            part = &part[b..];
        }
        out += &(kept + part);
    }
    out + &css[kit..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Look, Ui};

    #[test]
    fn the_look_follows_the_theme_mode_its_hue_and_layout() {
        // aio tests/app-theme.test.ts, standalone-theme-auto-app-css.test.ts.
        let ui = |theme, hue, layout| Ui { theme, hue, layout, ..Ui::default() };
        let full = sheet(&ui(Look::Full, None, true), "kit", false);
        assert_eq!(full, super::super::theme_css(hue("kit")), "the whole theme");
        assert_eq!(sheet(&ui(Look::Auto, None, true), "kit", false), full, "auto: the look while the app has no CSS");
        let tokens = sheet(&ui(Look::Tokens, None, true), "kit", false);
        assert_eq!(sheet(&ui(Look::Auto, None, true), "kit", true), tokens, "and steps aside when it has");
        assert!(tokens.contains("--aio-hue:") && tokens.contains(".aio-btn {"), "tokens and the kit's controls");
        assert!(!tokens.contains("body{") && !tokens.contains(":where(main)"), "nothing paints a bare element");
        assert_eq!(sheet(&ui(Look::None, None, true), "kit", false), "");
        assert!(sheet(&ui(Look::Full, Some(210), true), "kit", true).contains("--aio-hue:210;"), "the hue knob");
        let bare = sheet(&ui(Look::Full, None, false), "kit", false);
        assert!(!bare.contains(":where(main)") && !bare.contains(":where(.card)"), "no layout");
        assert!(
            bare.contains(":where(h1,h2,h3,h4,h5,h6)") && bare.contains("prefers-reduced-motion"),
            "the look stays"
        );
        assert!(bare.contains("/* ── the three environments ") && bare.contains(".aio-btn {"));
        assert_eq!(bare.matches('{').count(), bare.matches('}').count());
        let soft = Ui { neutral: crate::config::Neutral::Soft, ..Ui::default() };
        assert!(sheet(&soft, "kit", false).starts_with(&tokens_css(hue("kit"), true)), "[ui] neutral = \"soft\"");
    }

    #[test]
    fn the_hue_is_the_ids_and_the_same_on_every_machine() {
        assert_eq!((hue("notes"), hue("todo"), hue("contacts")), (226, 61, 40));
    }

    #[test]
    fn the_tokens_are_aios_byte_for_byte() {
        assert_eq!(tokens_css(hue("todo"), false), include_str!("../../tests/fixtures/aio-theme-tokens-61.css"));
        let green = tokens_css(115, false);
        assert!(green.contains("--aio-accent-ink:hsl(115 68% 30%);"), "solved per hue: {green}");
        assert!(green.contains("--aio-shadow-1:0 1px 2px #1b231a14;"));
    }

    #[test]
    fn a_blue_takes_white_ink_and_soft_neutrals_are_near_grey() {
        let blue = tokens_css(230, true);
        assert!(blue.contains("--aio-on-accent:#fff;"), "{blue}");
        assert!(blue.contains("--aio-text:hsl(230 4% 12%);") && blue.contains("--aio-surface-2:hsl(230 4% 97%);"));
        assert!(tokens_css(61, false).contains("--aio-on-accent:hsl(61 45% 11%);"), "a yellow keeps its colour");
    }

    /// Colour token `name`: the dark block's when `dark` and it sets one,
    /// else the light block's.
    fn token_in(css: &str, name: &str, dark: bool) -> Hsl {
        let (light, dark_block) = css.split_once("prefers-color-scheme: dark").unwrap();
        let key = format!("{name}:");
        let block = if dark && dark_block.contains(&key) { dark_block } else { light };
        let v = block[block.find(&key).unwrap() + key.len()..].split(';').next().unwrap();
        if v == "#fff" {
            return (0.0, 0.0, 1.0);
        }
        let n: Vec<f64> = v
            .trim_start_matches("hsl(")
            .trim_end_matches(')')
            .split(' ')
            .map(|p| p.trim_end_matches('%').parse().unwrap())
            .collect();
        (n[0], n[1] / 100.0, n[2] / 100.0)
    }

    #[test]
    fn labels_and_accent_text_clear_aa_on_every_surface_and_every_hue() {
        for soft in [false, true] {
            for h in 0..360 {
                let css = tokens_css(h, soft);
                let r = |a: Hsl, b: Hsl| contrast(luminance(a), luminance(b));
                let accent = token_in(&css, "--aio-accent", false);
                assert!(r(accent, token_in(&css, "--aio-on-accent", false)) >= 4.5, "hue {h}: the fill's label");
                for dark in [false, true] {
                    let ink = token_in(&css, "--aio-accent-ink", dark);
                    let surface = token_in(&css, "--aio-surface", dark);
                    for bg in ["--aio-bg", "--aio-surface", "--aio-surface-2"] {
                        assert!(r(ink, token_in(&css, bg, dark)) >= 4.5, "hue {h} dark {dark}: accent text on {bg}");
                    }
                    let tint = oklab_mix_luminance(accent, surface, 0.1);
                    assert!(contrast(luminance(ink), tint) >= 4.5, "hue {h} dark {dark}: accent text on the tint");
                    let text = r(token_in(&css, "--aio-text", dark), token_in(&css, "--aio-bg", dark));
                    assert!(text >= 7.0, "hue {h} dark {dark}: body text {text}");
                }
            }
        }
    }

    #[test]
    fn one_layer_no_alpha_on_a_colour_function_no_float_noise_and_real_shadows() {
        for h in [0, 61, 115, 230, 356] {
            let css = tokens_css(h, false);
            assert!(
                css.starts_with("@layer aio {") && css.matches("@layer").count() == 1,
                "one layer: the app's CSS wins"
            );
            assert_eq!(css.matches('{').count(), css.matches('}').count());
            let alpha =
                |l: &str| l.match_indices(')').any(|(i, _)| l[i + 1..].starts_with(|c: char| c.is_ascii_hexdigit()));
            assert!(!css.lines().any(alpha), "a colour function with an alpha suffix");
            let noisy = |l: &str| {
                l.split(|c: char| !(c.is_ascii_digit() || c == '.'))
                    .any(|n| n.split_once('.').is_some_and(|(_, f)| f.len() >= 4))
            };
            assert!(!css.lines().any(noisy), "float noise");
            let shadows: Vec<&str> = css
                .match_indices("--aio-shadow-")
                .map(|(i, _)| css[i..].split_once(':').unwrap().1.split(';').next().unwrap())
                .collect();
            assert_eq!(shadows.iter().filter(|s| **s == "none").count(), 2, "off for prefers-contrast: more");
            for layer in shadows.iter().filter(|s| **s != "none").flat_map(|s| s.split(',')) {
                let w: Vec<&str> = layer.split(' ').collect();
                assert!(
                    (4..=5).contains(&w.len()) && w[..w.len() - 1].iter().all(|v| *v == "0" || v.ends_with("px")),
                    "{layer}"
                );
                assert!(
                    w[w.len() - 1].starts_with('#') && w[w.len() - 1][1..].chars().all(|c| c.is_ascii_hexdigit()),
                    "{layer}"
                );
            }
        }
    }
}

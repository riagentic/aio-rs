# Theming and styles

An app's look is plain CSS: the kit's theme first (its `--aio-*` tokens,
the look of plain elements and the kit's controls), then the app's own
stylesheets. Under `ax dev` a saved stylesheet shows at once, with no
compile. Feature `kit`.

- **Use the theme** to get a readable, accessible app from one hue, and
  the kit's controls styled.
- **Bring your own CSS** on top (it wins: the theme sits in `@layer aio`),
  or adopt the theme as a file of the app's own, or leave aio's CSS out.

## The page's CSS

```rust,ignore
// ax create's template (src/lib.rs), compiled by ax's own tests:
// The app's one hue (`[ui] hue`, else its id's): its icon's and splash's too.
let theme = aio::kit::theme_css(aio::hue!());
style { {aio::use_css!(aio::ui::scheme(), &theme; "assets/style.css")} }
```

`aio::use_css!(scheme, made…; "file.css", …)` makes the page's stylesheet:

- Sheets made in code go before the `;` and come first (here the kit's
  theme); files follow, paths from the package's folder.
- A dev build re-reads a saved file and redraws within 0.2 s, no compile;
  in a browser under `ax dev --web` the page asks the dev server. A
  release build has the files built in (`include_str!`) and reads none.
- It is made on every render: never keep CSS in `once` (made once,
  stale after an edit).
- `scheme` is light or dark: `aio::ui::scheme()` (`[ui] scheme`: the
  OS's by default), or the app's own `bool` (below).

`aio::ui::page_css(&[sheets], scheme)` is the same without the live
reload, for CSS an app builds itself.

## One hue

```rust
use aio::config::{Look, Ui};
use aio::kit::theme;

# fn main() {
// Every --aio-* colour comes from one hue, hashed from the app's id.
let hue = theme::hue("notes");
assert_eq!(hue, theme::hue("notes"), "the same on every machine");
let tokens = theme::tokens_css(hue, false);
assert!(tokens.starts_with("@layer aio") && tokens.contains("--aio-accent:"));

// [ui] in aio.toml, as the kit's sheet reads it.
let ui = Ui { hue: Some(210), ..Ui::default() };
let css = theme::sheet(&ui, "notes", false);
assert!(css.contains("--aio-hue:210;"));
assert_eq!(theme::sheet(&Ui { theme: Look::None, ..ui }, "notes", false), "", "no aio CSS at all");
# }
```

- `aio::kit::theme::hue(id)`: the hue of an app id (aio's `appHue`: hashed
  over the slug the id makes, so `My Notes` and `my-notes` are one colour;
  a rename recolours, a new machine doesn't).
- `aio::hue!()`: the app's own hue as it builds (`Config::hue`: `[ui] hue`,
  else its id's), the one its page, icon, splash and download window share.
- `aio::kit::theme::tokens_css(hue, soft)`: the tokens alone, light and
  dark; `soft` keeps the neutrals near grey (the accent keeps the hue).
  The inks are solved for contrast against that hue, so text stays
  readable (4.6:1) whatever it is.
- `aio::kit::theme_css(hue)`: the tokens and the kit's whole stylesheet,
  what the template passes.
- `aio::kit::theme::sheet(&cfg.ui, &cfg.app.id, ships_css)`: what `[ui]`
  in `aio.toml` asks for (below), `[ui] neutral = "soft"` included
  (aio's `"neutral": "soft"`). Pass it instead of `theme_css` to let
  `aio.toml` decide.

## `[ui]` in `aio.toml`

```toml
[app]
id = "notes"

[ui]
theme = "auto"   # tokens | auto | full | none
hue = 210        # the accent's hue in degrees; unset: hashed from the id
layout = true    # <main> as a centred page, and .card .row .stack .grid
```

| `theme` | The page gets |
|---|---|
| `tokens` | the tokens and the kit's controls; plain elements look as the browser draws them |
| `auto` (default) | the whole look while the app ships no stylesheet of its own (`ships_css`), then `tokens` |
| `full` | the whole look, the app's stylesheet over it |
| `none` | no aio CSS at all, not even its base rules: the app brings every rule, and the kit is bare |

`layout = false` keeps how elements look and drops the page layout:
`<main>` as a centred page and the classes `.card`, `.stack`, `.row`,
`.grid`, `.muted`, `.badge`.

## What the look styles

With `theme = "full"` (or `auto` while the app ships no CSS), plain
semantic HTML is already styled, so the markup an app writes anyway looks
finished:

- type: headings, paragraphs, links, lists, `blockquote`, `hr`, `code`,
  `pre`, `kbd`;
- controls: `button` (quiet by default; `class: "primary"` and a
  `type="submit"` button take the accent, `ghost` and `danger` the other
  two), `input`, `select`, `textarea`, checkboxes and radios, `label`,
  `fieldset`, `legend`, `progress`;
- data: `table` (rows light up under the pointer), `img`, `video`,
  `canvas`, `svg` (never wider than their box), `dialog`, `details`;
- the page: `<main>` is a centred page `--aio-page` (72rem) wide, padded;
  a `<header>` or `<footer>` at the top level is a full-bleed bar whose
  content lines up with `<main>`. An app that wants the full bleed (a
  canvas, a map) doesn't write `<main>`.

| Class | Does |
|---|---|
| `.card` | surface, border, radius, soft shadow |
| `.stack` | a column with a gap |
| `.row` | a wrapping row, centred, with a gap |
| `.grid` | a responsive grid (columns of at least 15rem) |
| `.muted` | the quiet ink |
| `.badge` | a small accent pill |

Every rule sits in `@layer aio` and most are written with `:where(..)`,
so any rule of the app's wins over it, even a bare `button { .. }`. Each is
written in logical properties (`margin-inline`, `text-align: start`), so
`[app] dir = "rtl"` mirrors the whole look.

The look also answers three settings a person makes on their device:

| Setting | The look |
|---|---|
| a coarse pointer (a phone) | buttons and fields at least 44 px tall |
| `prefers-contrast: more` | borders and quiet ink become the text colour, no shadows (in `tokens` mode too), thicker focus rings and borders |
| `forced-colors: active` (Windows high contrast) | borders where backgrounds separated, the focus ring in the system's highlight |
| `prefers-reduced-motion` | no transitions |

| You want | Do |
|---|---|
| a finished look, no CSS to write | nothing: `theme = "auto"` |
| your own design, the kit's controls in your colours | ship `assets/style.css`; `auto` steps down to `tokens` |
| no aio CSS at all | `theme = "none"` |
| aio's look, frozen, to build on | `ax theme adopt` |
| aio's look, following aio-rs as it changes, with your CSS over it | `theme = "full"` |

## Tokens

The tokens are custom properties on `:root`; set one in your own CSS to
change everything made from it.

| Token | Is | Default |
|---|---|---|
| `--aio-hue` | the hue every colour is made from | from the app's id, or `[ui] hue` |
| `--aio-accent`, `--aio-on-accent` | the accent fill, the text on it | solved from the hue |
| `--aio-accent-ink` | the accent as text (a link, an active tab), readable on every background | solved, 4.6:1 |
| `--aio-bg`, `--aio-surface`, `--aio-surface-2`, `--aio-tint` | the page, a card, a raised part, an accent-tinted surface | near white / near black, a touch of the hue |
| `--aio-text`, `--aio-muted`, `--aio-border` | ink, quiet ink, lines | |
| `--aio-ring` | the focus ring (a `box-shadow`) | 3 px of the accent |
| `--aio-ok`, `--aio-warn`, `--aio-danger`, `--aio-on-danger` | status colours; the ink on danger is solved from the fill | green, amber, red |
| `--aio-s-1` … `--aio-s-6` | spacing | .25, .5, .75, 1, 1.5, 2.5 rem |
| `--aio-r-1` … `--aio-r-4` | radii | 6, 10, 14, 20 px |
| `--aio-page` | `<main>`'s width | 72rem |
| `--aio-font`, `--aio-mono` | type | the system's UI and mono fonts |
| `--aio-shadow-1`, `--aio-shadow-2` | a hairline shadow, a raised one | tinted by the hue |

The dark side of every colour token is in the same sheet, under
`prefers-color-scheme: dark` (which `use_css!`'s `dark` turns on or off:
below).

### The kit's own tokens

The kit's controls read `--aio-ui-*` tokens, each defaulting to the
theme's, so a theme-wide change reaches them and a kit-only one reaches
nothing else:

| Kit token | Defaults to | Colours |
|---|---|---|
| `--aio-ui-accent` | `--aio-accent` | a primary fill, a focus ring |
| `--aio-ui-on-accent` | `--aio-on-accent` | the ink on that fill |
| `--aio-ui-accent-ink` | `--aio-accent-ink` | the accent as text (an active tab, a Markdown link) |
| `--aio-ui-ink`, `--aio-ui-ink-soft` | `--aio-text`, `--aio-muted` | text, quiet text |
| `--aio-ui-bg` | `--aio-surface` | a field's background |
| `--aio-ui-surface` | `--aio-surface-2` | a secondary button |
| `--aio-ui-line` | `--aio-border` | borders |
| `--aio-ui-danger`, `--aio-ui-on-danger` | `--aio-danger`, `--aio-on-danger` | a destructive action |
| `--aio-ui-radius` | `--aio-r-2` | corners |
| `--aio-ui-font` | `--aio-font` | the controls' typeface |

```css
/* assets/style.css: an unlayered rule beats the theme without !important */
:root {
  --aio-accent: #7c3aed;     /* a rebrand sets the fill… */
  --aio-accent-ink: #6d28d9; /* …and the accent as text, readable on the page */
  --aio-ui-radius: 12px;     /* the kit's corners only */
}
.toolbar { display: flex; gap: var(--aio-s-2); border-bottom: 1px solid var(--aio-border); }
```

The colours are computed in Rust from the hue when the sheet is made, so
setting `--aio-hue` in CSS changes nothing by itself: change the hue with
`[ui] hue` (all colours re-solved), or set the colour tokens you want.

The kit's classes come with the theme: `aio::ui::btn("primary")` is
`aio-btn aio-btn--primary aio-btn--md` (variants `primary`, `secondary`,
`ghost`, `danger`), `aio::ui::input_cls(extra, invalid)` an input's. A
kit control on a page that nothing styles shows as bare HTML, and a dev
build says so once in the log.

## Dark mode

By default the page follows the OS, as it changes (aio's automatic dark
mode): `[ui] scheme = "system"` in `aio.toml`, what `aio::ui::scheme()`
gives the template. The sheets stay as written, so every
`@media (prefers-color-scheme: dark)` rule and `light-dark(a, b)` follows
the OS with no render. `scheme = "light"` or `"dark"` fixes one. Code
that draws its own colours (a canvas, a chart) reads
`aio::ui::use_dark()`, a `Memo<bool>` that follows the OS too. A test
turns the OS dark with `ui.set_os_dark(true)` (or mounts dark:
`aio::testing::Mount::default().dark()`).

A `bool` instead of the scheme is the app's own choice: `true` applies
the sheets' dark rules and the dark side of every `light-dark(a, b)`,
`false` the light ones, whatever the OS says (a phone in dark mode never
darkens half of a light app). To offer a choice, keep it in a cell and
pass it:

```rust
use aio::ui::prelude::*;

#[aio::cell(persist, id = "look")]
pub struct Look {
    pub dark: bool,
}

#[aio::methods]
impl Look {
    pub fn set_dark(&mut self, dark: bool) {
        self.dark = dark;
    }
}

aio::client!(Look, look);

#[component]
fn App() -> View {
    use_live();
    let dark = part(|l| &l.dark).get();
    let theme = aio::kit::theme_css(aio::kit::theme::hue("notes"));
    rsx! {
        style { {aio::ui::page_css(&[&theme, "body { font-size: 15px }"], dark)} }
        aio::kit::Switch { checked: dark, label: "Dark", "data-t": "dark",
            onchange: move |on| _ = aio::act!(set_dark(on)) }
        p { "data-t": "text", "Notes" }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
let light = ui.style("text", "color");
ui.click("dark");
assert!(ui.checked("dark"));
assert_ne!(ui.style("text", "color"), light, "the app's switch, not the OS, turned the text light");
# }
```

How it works: `aio::ui::page_css` (and `use_css!`) rewrite the sheets
for the app's choice: each `@media (prefers-color-scheme: dark)` becomes
`@media all` when `dark`, `@media not all` when not, and each
`light-dark(a, b)` its side; token rules on `:root` also apply to a
`[data-theme]` element. So an app's own dark rules, written the usual
way, follow the app's switch too, and a dark OS never darkens half of a
light app.

On Android, `[build.android] color_scheme = "light" | "dark"` fixes the
system bars and WebView whatever the phone's setting (unset: the
phone's). `aio_shell::Window { background: Some(["#fff", "#111"]), .. }`
is what shows before the first paint.

## Checking contrast

Every `TestUi` mount audits the page as aio's dev build did: text whose
colour over its backdrop is under WCAG AA (4.5:1, 3:1 for large text) is
said in the test's output. `TestUi::contrast()` returns the findings, to
fail on them:

```rust
use aio::ui::prelude::*;

#[component]
fn App() -> View {
    rsx! {
        p { style: "color: #111; background: #fff", "Readable" }
        p { style: "color: #ccc; background: #fff", "Faint" }
        // A backdrop the audit can't see (text over a video): skipped.
        div { "data-aio-contrast": "skip",
            p { style: "color: #eee; background: #fff", "Over the video" }
        }
    }
}

# fn main() {
let ui = aio::testing::TestUi::new(App);
let bad = ui.contrast();
assert_eq!(bad.len(), 1, "{bad:?}");
assert!(bad[0].contains("Faint") && bad[0].contains("needs 4.5:1"));
# }
```

A backdrop with an image can't be judged and is skipped. Only the exact
value `"skip"` counts. `ui.shot()` draws the page as a PNG to look at.

## Your own copy: `ax theme adopt`

`ax theme adopt <app>` writes the theme for the app's hue and `[ui]` into
`assets/aio-theme.css`: from then on the app's own file, in its git
history, edited like any stylesheet, and no aio-rs upgrade moves its
look. It keeps `@layer aio`, so the app's unlayered rules still beat it
without `!important`: override a piece without deleting it. Adopting is
for building on aio's look and keeping it still; `theme = "full"` is the
living version, which moves when aio-rs's look does. Pass it first instead of `theme_css`:
`aio::use_css!(false; "assets/aio-theme.css", "assets/style.css")`.
`--force` writes over an adopted file.

## A CSS toolchain: `[build] css`

```toml
[app]
id = "notes"

[build]
css = "tailwindcss -i src/app.css -o assets/style.css"
```

The command (split on spaces, or an argv list; never through a shell)
runs in the app's folder before every build and on every change under
`ax dev`, so what `include_str!` embeds is fresh. It fails loud: a build
stops on it, `ax dev` says so and keeps the running app. Tailwind, PostCSS
and Sass all fit; point their output at a file `use_css!` names.

### Tailwind and the theme together

The tokens are plain custom properties, so Tailwind v4 reads them:

```css
/* src/app.css */
@import "tailwindcss";

@theme {
  --color-accent: var(--aio-accent);
  --color-surface: var(--aio-surface);
  --color-ink: var(--aio-text);
}
```

Shipping that output as `assets/style.css` makes `theme = "auto"` step
down to `tokens`: the colours and the kit stay, the element look goes, and
Tailwind's own reset takes its place.

### Class names

The page has one stylesheet, so class names are global: prefix your
own by component (`.timeline-track`, not `.track`). Two components that
pick the same name share rules, and the symptom is visual, which no test
sees. Classes starting `aio-` are the kit's.

## Right to left

`[app] dir = "rtl"` (or `auto`) sets `<html dir>`; `[app] lang` sets
`<html lang>`; the browser lays the page out from it.
`aio::kit::Direction { rtl: true, .. }` turns a part of the page around
and tells the kit: `Tabs` inside it (or with `rtl: true`) swaps its arrow
keys.

## Gaps against aio

- **Scoped classes.** aio's `css` tag makes a class from a hash of its
  rule, and `aiol` warns when two stylesheets define one class
  differently. On air `aio::ui::css("color: red; &:hover { … }")` is
  that class (FNV-1a of the rule, as aio's), its rule put in the page once
  per window and scoped by native nesting (`@keyframes` lifted out), and
  `aio::ui::collect_css()` gives every rule; no warning, since a class is
  its rule's hash. On Dioxus: prefix class names by hand.
- **The dev contrast check in a running app.** aio warns in the dev
  page's console; aio-rs audits in `TestUi` (above), not in `ax dev`.

## Common mistakes

- A kit control with no theme on the page: pass `theme_css(..)` (or the
  adopted file) first; the dev log names the control.
- Expecting the OS's dark mode: the app passes `dark`.
- `!important` to beat the theme: not needed, an unlayered rule wins.
- `[ui] theme = "none"` and the kit: the controls are then bare, by design.
- CSS in `once`: stale after an edit (above).
- Setting `--aio-hue` in CSS and expecting new colours: the colours were
  solved from the hue in Rust; use `[ui] hue`.
- A rebrand that sets `--aio-accent` but not `--aio-accent-ink`: links
  and the active tab keep the old colour as text.
- A generic class name (`.title`, `.row` of your own): it meets the look's
  `.row` or another component's rule.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::use_css!`,
`aio::ui::page_css`, `aio::ui::btn`, `aio::kit::theme`,
`aio::kit::theme_css`, `aio::config::Ui`.

//! `Canvas`: a 2-D drawing made in Rust, no JS in the app. A [`Draw`] list
//! (rects, lines, paths, text, images, pixels, transforms, fills and
//! strokes: the page's `CanvasRenderingContext2D`, by the same names) goes
//! to one small painter in the page, an [`Island`]: it paints at the
//! device's pixel ratio, at most once a frame, again when the list, the
//! box's size or the page's colours change, and sends pointer events back
//! in the drawing's coordinates. An app that draws none builds none of it.
use super::Align;
use super::island::{Island, WebIsland};
use crate::ui::prelude::*;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The painter, its comment lines left out.
const PAINTER: Island = Island::js(crate::shrunk!(js, include_str!("canvas.js")));

/// A drawing: the canvas's calls in order, from a clean state (the box's
/// text colour as fill and stroke, its font, 1 px lines, text left-aligned
/// on its middle). Colours are CSS colours, `var(--aio-accent)` too: the
/// painter reads a token as the page has it now, again when the theme
/// changes. Each call returns the list, so calls chain.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Draw(Vec<Value>);

impl Draw {
    /// An empty drawing.
    pub fn new() -> Self {
        Self::default()
    }

    fn op(&mut self, v: Value) -> &mut Self {
        self.0.push(v);
        self
    }

    /// Whether it draws nothing.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Fill with `color` from now on.
    pub fn fill(&mut self, color: &str) -> &mut Self {
        self.op(json!(["F", color]))
    }

    /// Stroke with `color`, `width` wide, from now on.
    pub fn stroke(&mut self, color: &str, width: f64) -> &mut Self {
        self.op(json!(["S", color, width]))
    }

    /// Draw at opacity `a` (0 to 1) from now on.
    pub fn alpha(&mut self, a: f64) -> &mut Self {
        self.op(json!(["A", a]))
    }

    /// Text in `css` (`"bold 12px sans-serif"`) from now on.
    pub fn font(&mut self, css: &str) -> &mut Self {
        self.op(json!(["T", css]))
    }

    /// Text from now on starting at its `x`, centred on it, or ending at it.
    pub fn align(&mut self, a: Align) -> &mut Self {
        let a = ["start", "center", "end"][a as usize];
        self.op(json!(["L", a]))
    }

    /// Strokes dashed (`&[4.0, 2.0]`: 4 on, 2 off), solid with `&[]`.
    pub fn dash(&mut self, pattern: &[f64]) -> &mut Self {
        self.op(json!(["D", pattern]))
    }

    /// Images scaled smoothly (`true`, the start) or as blocks (`false`: a
    /// heat map's cells, pixel art).
    pub fn smooth(&mut self, on: bool) -> &mut Self {
        self.op(json!(["q", on]))
    }

    /// A filled rectangle.
    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) -> &mut Self {
        self.op(json!(["r", x, y, w, h]))
    }

    /// A rectangle's outline.
    pub fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64) -> &mut Self {
        self.op(json!(["R", x, y, w, h]))
    }

    /// A stroked line from `(x1, y1)` to `(x2, y2)`.
    pub fn line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) -> &mut Self {
        self.op(json!(["l", x1, y1, x2, y2]))
    }

    /// A stroked line through `points`, sent as one flat list.
    pub fn polyline(&mut self, points: &[(f64, f64)]) -> &mut Self {
        let flat: Vec<f64> = points.iter().flat_map(|&(x, y)| [x, y]).collect();
        self.op(json!(["p", flat]))
    }

    /// The path: start a figure at `(x, y)`. A path is the calls since the
    /// last [`fill_path`](Self::fill_path) or [`stroke_path`](Self::stroke_path);
    /// `rect`, `line`, `polyline` and `text` leave it be.
    pub fn move_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.op(json!(["m", x, y]))
    }

    /// The path: a straight line to `(x, y)`.
    pub fn line_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.op(json!(["t", x, y]))
    }

    /// The path: a cubic Bézier curve to `(x, y)` through control points 1 and 2.
    pub fn curve_to(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) -> &mut Self {
        self.op(json!(["b", c1x, c1y, c2x, c2y, x, y]))
    }

    /// The path: an arc around `(cx, cy)`, radius `r`, from angle `a0` to
    /// `a1` (radians, clockwise from 3 o'clock); `0` to `TAU` is a circle.
    pub fn arc(&mut self, cx: f64, cy: f64, r: f64, a0: f64, a1: f64) -> &mut Self {
        self.op(json!(["a", cx, cy, r, a0, a1]))
    }

    /// The path: close the figure back to its start.
    pub fn close(&mut self) -> &mut Self {
        self.op(json!(["z"]))
    }

    /// Fill the path, then start a new one.
    pub fn fill_path(&mut self) -> &mut Self {
        self.op(json!(["f"]))
    }

    /// Stroke the path, then start a new one.
    pub fn stroke_path(&mut self) -> &mut Self {
        self.op(json!(["s"]))
    }

    /// `text` in the fill colour, placed on `x` as [`align`](Self::align) says
    /// and centred on `y`.
    pub fn text(&mut self, x: f64, y: f64, text: &str) -> &mut Self {
        self.op(json!(["x", x, y, text]))
    }

    /// The image at `src` (an asset's address, a `data:` URL) in the box
    /// `x, y, w, h`. It shows once loaded (the canvas paints again then).
    pub fn image(&mut self, src: &str, x: f64, y: f64, w: f64, h: f64) -> &mut Self {
        self.op(json!(["i", src, x, y, w, h]))
    }

    /// Pixels made in the app, stretched over `x, y, w, h`: one bitmap
    /// blit, however many pixels. The page decodes a `Pixels` once and
    /// keeps it while it is in the list, so repaints (a resize, an overlay
    /// drawn on top) cost a blit.
    pub fn pixels(&mut self, px: &Pixels, x: f64, y: f64, w: f64, h: f64) -> &mut Self {
        self.op(json!(["g", px.w, px.h, px.data, px.palette, x, y, w, h]))
    }

    /// Keep the transform, styles and clip until [`restore`](Self::restore).
    pub fn save(&mut self) -> &mut Self {
        self.op(json!(["("]))
    }

    /// Back to the last [`save`](Self::save).
    pub fn restore(&mut self) -> &mut Self {
        self.op(json!([")"]))
    }

    /// Move the origin by `(x, y)`.
    pub fn translate(&mut self, x: f64, y: f64) -> &mut Self {
        self.op(json!(["o", x, y]))
    }

    /// Scale what follows by `(x, y)`.
    pub fn scale(&mut self, x: f64, y: f64) -> &mut Self {
        self.op(json!(["k", x, y]))
    }

    /// Turn what follows by `a` radians, clockwise.
    pub fn rotate(&mut self, a: f64) -> &mut Self {
        self.op(json!(["w", a]))
    }
}

/// A bitmap made in the app, `w` × `h`, rows top to bottom, for
/// [`Draw::pixels`]. Bytes it lacks draw transparent.
#[derive(Clone, Debug, PartialEq)]
pub struct Pixels {
    w: u32,
    h: u32,
    data: String,
    palette: Option<String>,
}

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

impl Pixels {
    /// Four bytes a pixel: red, green, blue, alpha.
    pub fn rgba(w: u32, h: u32, rgba: &[u8]) -> Self {
        Self { w, h, data: B64.encode(rgba), palette: None }
    }

    /// One byte a pixel, an index into `palette` (RGBA, at most 256): a
    /// quarter of the bytes to send. An index past its end is transparent.
    pub fn indexed(w: u32, h: u32, index: &[u8], palette: &[[u8; 4]]) -> Self {
        let lut: Vec<u8> = palette.iter().take(256).flatten().copied().collect();
        Self { w, h, data: B64.encode(index), palette: Some(B64.encode(lut)) }
    }
}

/// What a pointer did on a [`Canvas`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PointerKind {
    /// A button went down (the canvas keeps the pointer until it is up).
    Down,
    /// It moved: at most one a frame.
    Move,
    /// A button came up.
    Up,
    /// It left the canvas.
    Leave,
    /// A click: down and up on the canvas.
    Click,
}

/// A pointer event on a [`Canvas`], at `(x, y)` in the drawing's
/// coordinates (its `view`'s, else CSS pixels from its top left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasPointer {
    pub kind: PointerKind,
    pub x: f64,
    pub y: f64,
    /// The buttons held (`1` the main one, `2` the second, `4` the middle).
    pub buttons: u16,
}

/// What the painter sends: a pointer's event, or the box's size.
#[derive(Deserialize)]
#[serde(tag = "t")]
enum Said {
    #[serde(rename = "p")]
    Pointer { k: PointerKind, x: f64, y: f64, b: u16 },
    #[serde(rename = "s")]
    Size { w: f64, h: f64 },
}

#[derive(Clone, PartialEq, Serialize)]
struct Paint {
    d: Draw,
    v: Option<(f64, f64)>,
    e: bool,
    s: bool,
}

/// A canvas filling its box (size it with `class` or `style`; 150 px tall
/// by default), painted from `draw`: a new list repaints it.
/// - `view`: the drawing's own width and height, stretched over the box
///   (an SVG `viewBox` with `preserveAspectRatio="none"`): a grid of cells
///   drawn in cell units. Unset: CSS pixels.
/// - `onpointer`: down, move (one a frame), up, leave and click, in the
///   drawing's coordinates. Unset, the page sends none.
/// - `onsize`: the box's size in CSS pixels, at first and on each change,
///   to lay a drawing out for it.
///
/// `canvas.aioPaintMs` in the page is the last paint's time, for profiling.
#[component]
pub fn Canvas(
    draw: Prop<Draw>,
    #[props(default)] view: Prop<Option<(f64, f64)>>,
    onpointer: Option<EventHandler<CanvasPointer>>,
    onsize: Option<EventHandler<(f64, f64)>>,
    #[props(default, into)] class: Prop<String>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    let said = EventHandler::new(move |m: Said| match m {
        Said::Pointer { k, x, y, b } => {
            if let Some(f) = onpointer {
                f.call(CanvasPointer { kind: k, x, y, buttons: b })
            }
        }
        Said::Size { w, h } => {
            if let Some(f) = onsize {
                f.call((w, h))
            }
        }
    });
    rsx! {
        WebIsland::<Paint, Said> {
            island: PAINTER,
            props: Paint { d: draw.get(), v: view.get(), e: onpointer.is_some(), s: onsize.is_some() },
            onevent: said,
            class: super::join("aio-canvas", &class.get()),
            ..attributes,
        }
    }
}

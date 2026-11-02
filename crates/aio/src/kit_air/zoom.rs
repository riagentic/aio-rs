//! A zoomable, pannable viewport (aio's `ZoomPan`): the wheel zooms at the
//! cursor, a drag pans (and the click that ends it doesn't reach the
//! content), even with the pointer out of the box (a window-wide layer takes
//! it while dragging: aio's pointer capture), the buttons zoom about the
//! centre or fit. The camera math is
//! pure, for tests without a DOM.
use super::join;
use crate::ui::prelude::*;
use std::rc::Rc;

/// The camera: content point (0, 0) sits at (`x`, `y`) in the viewport,
/// drawn `scale` times its size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomView {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
}

impl Default for ZoomView {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, scale: 1.0 }
    }
}

/// `view` zoomed by `factor`, viewport point (`px`, `py`) (the cursor) kept
/// over the same content point; the scale clamped to `min..=max`.
pub fn zoom_at(view: ZoomView, factor: f64, (px, py): (f64, f64), (min, max): (f64, f64)) -> ZoomView {
    let scale = (view.scale * factor).clamp(min, max);
    let k = scale / view.scale;
    ZoomView { x: px - (px - view.x) * k, y: py - (py - view.y) * k, scale }
}

/// The view that shows `content` (width, height) whole and centred in
/// `viewport`, `pad` from its edges, the scale clamped to `min..=max`.
pub fn fit_view(content: (f64, f64), viewport: (f64, f64), pad: f64, (min, max): (f64, f64)) -> ZoomView {
    if !(content.0 > 0.0 && content.1 > 0.0) {
        return ZoomView::default();
    }
    let scale = ((viewport.0 - 2.0 * pad) / content.0).min((viewport.1 - 2.0 * pad) / content.1).clamp(min, max);
    ZoomView { x: (viewport.0 - content.0 * scale) / 2.0, y: (viewport.1 - content.1 * scale) / 2.0, scale }
}

/// How far a press may move and still be a click, in px.
const SLOP: f64 = 4.0;
const STEP: f64 = 1.25;

/// A press being dragged: where it started, the view then, and its pointer.
#[derive(Clone, Copy)]
struct Press {
    at: (f64, f64),
    from: ZoomView,
    id: i32,
    dragged: bool,
}

/// The viewport; size it with `class` or `style` (it fills its box, `overflow: hidden`).
#[component]
pub fn ZoomPan(
    #[props(default = 0.1)] min_scale: f64,
    #[props(default = 10.0)] max_scale: f64,
    #[props(default)] initial: ZoomView,
    onview: Option<EventHandler<ZoomView>>,
    /// The ＋ / － / fit buttons.
    #[props(default = true)]
    controls: bool,
    #[props(default, into)] class: Prop<String>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: View,
) -> View {
    let view = signal(|| initial);
    let press = signal(|| None::<Press>);
    let boxed = signal(|| None::<Rc<MountedData>>);
    let stage = signal(|| None::<Rc<MountedData>>);
    let range = (min_scale, max_scale);
    let set = move |v: ZoomView| {
        view.set(v);
        if let Some(f) = onview {
            f.call(v)
        }
    };
    let size = |el: Option<Rc<MountedData>>| async move {
        match el {
            Some(el) => el.get_client_rect().await.map(|r| (r.width(), r.height())).unwrap_or_default(),
            None => (0.0, 0.0),
        }
    };
    let zoom = move |factor: f64| async move {
        let (w, h) = size(boxed.read_untracked().clone()).await;
        let now = *view.read_untracked();
        set(zoom_at(now, factor, (w / 2.0, h / 2.0), range));
    };
    let fit = move |_| async move {
        let (outer, inner) = (size(boxed.read_untracked().clone()).await, size(stage.read_untracked().clone()).await);
        // The stage is drawn scaled: its unscaled size is what fits.
        let s = view.read_untracked().scale;
        set(fit_view((inner.0 / s, inner.1 / s), outer, 16.0, range));
    };
    let dragging = memo(move || press.with(|p| p.is_some_and(|p| p.dragged)));
    rsx! {
        div {
            class: join("aio-zoompan", &class.get()),
            onmounted: move |e| *boxed.write() = Some(e.data()),
            onwheel: move |e: WheelEvent| {
                e.prevent_default();
                let p = e.element_coordinates();
                let now = *view.read_untracked();
                set(zoom_at(now, (-e.delta().strip_units().y * 0.0015).exp(), (p.x, p.y), range));
            },
            onpointerdown: move |e: PointerEvent| {
                if e.trigger_button() == Some(MouseButton::Primary) {
                    let c = e.client_coordinates();
                    *press.write() = Some(Press { at: (c.x, c.y), from: *view.read_untracked(), id: e.pointer_id(), dragged: false });
                }
            },
            onpointermove: move |e: PointerEvent| {
                let Some(mut p) = *press.read_untracked() else { return };
                if e.pointer_id() != p.id {
                    return;
                }
                // No button held: the release happened outside; the drag is over.
                if !e.held_buttons().contains(MouseButton::Primary) {
                    *press.write() = None;
                    return;
                }
                let c = e.client_coordinates();
                let (dx, dy) = (c.x - p.at.0, c.y - p.at.1);
                if !p.dragged && dx.hypot(dy) < SLOP {
                    return;
                }
                p.dragged = true;
                *press.write() = Some(p);
                set(ZoomView { x: p.from.x + dx, y: p.from.y + dy, ..p.from });
            },
            onpointerup: move |_| *press.write() = None,
            onpointercancel: move |_| *press.write() = None,
            ..attributes,
            div {
                class: "aio-zoompan__stage",
                "data-t": "zoomStage",
                style: view.with(|v| format!("transform:translate({}px,{}px) scale({})", v.x, v.y, v.scale)),
                onmounted: move |e| *stage.write() = Some(e.data()),
                {children}
            }
            if dragging.get() {
                // Pointer capture: every move and the release land here (and bubble up), wherever
                // the pointer is, so the click that ends the drag isn't the content's.
                div { class: "aio-zoompan__capture" }
            }
            if controls {
                div { class: "aio-zoompan__controls",
                    button { r#type: "button", class: "aio-zoompan__btn", "aria-label": "Zoom in", "data-t": "zoomIn", onclick: move |_| zoom(STEP), "+" }
                    button { r#type: "button", class: "aio-zoompan__btn", "aria-label": "Zoom out", "data-t": "zoomOut", onclick: move |_| zoom(1.0 / STEP), "−" }
                    button { r#type: "button", class: "aio-zoompan__btn", "aria-label": "Fit", "data-t": "zoomFit", onclick: fit, "⤢" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: (f64, f64) = (0.1, 10.0);

    #[test]
    fn zooming_keeps_the_point_under_the_cursor_and_clamps() {
        let v = zoom_at(ZoomView::default(), 2.0, (100.0, 50.0), R);
        assert_eq!(v, ZoomView { x: -100.0, y: -50.0, scale: 2.0 });
        // The content point under the cursor is the same before and after.
        let before = ((100.0 - 0.0) / 1.0, (50.0 - 0.0) / 1.0);
        assert_eq!(((100.0 - v.x) / v.scale, (50.0 - v.y) / v.scale), before);
        assert_eq!(zoom_at(v, 100.0, (0.0, 0.0), R).scale, 10.0);
        assert_eq!(zoom_at(v, 0.001, (0.0, 0.0), R).scale, 0.1);
    }

    #[test]
    fn fitting_centres_the_content_whole_inside_the_padding() {
        let v = fit_view((200.0, 100.0), (432.0, 432.0), 16.0, R);
        assert_eq!(v, ZoomView { x: 16.0, y: 116.0, scale: 2.0 });
        assert_eq!(fit_view((0.0, 10.0), (100.0, 100.0), 16.0, R), ZoomView::default(), "nothing to fit");
        assert_eq!(fit_view((1.0, 1.0), (1000.0, 1000.0), 0.0, R).scale, 10.0, "clamped");
    }
}

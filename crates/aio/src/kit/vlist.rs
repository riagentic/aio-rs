//! A long list rendered a window at a time (aio's `useVirtualList`): only the
//! rows the scroll position shows, and a few more each side, are elements.
use super::join;
use crate::ui::prelude::*;
use geometry::PixelsVector2D;
use std::ops::Range;
use std::rc::Rc;

/// The rows to render: the ones a `height` px viewport scrolled to `top`
/// shows (also the two it cuts at its edges), `overscan` more each side. The
/// start is clamped at both ends, so a list that shrank under its scroll
/// shows its last window, never a blank one. A row is at least 1 px.
pub fn window(len: usize, row: f64, height: f64, overscan: usize, top: f64) -> Range<usize> {
    let (row, top) = (row.max(1.0), top.max(0.0));
    let count = ((top % row + height.max(0.0)) / row).ceil() as usize + 2 * overscan;
    let start = ((top / row).floor() as usize).saturating_sub(overscan).min(len.saturating_sub(count));
    start..len.min(start + count)
}

/// `len` rows of `row_height` px in a `height` px box that scrolls; `row`
/// renders the row at an index. A scroll within the rows it has drawn
/// changes nothing (aio re-anchored its window only when it moved): the
/// list of rows is keyed by index, so a scroll that moves the window adds
/// the rows that came in and drops the ones that left, no other. Changing
/// `scroll_to` scrolls that row to the top (aio's `scrollToIndex`).
#[component]
pub fn VirtualList(
    len: Prop<usize>,
    row_height: f64,
    height: Prop<f64>,
    #[props(default = 3)] overscan: usize,
    scroll_to: Prop<Option<usize>>,
    row: Callback<usize, View>,
    #[props(default, into)] class: Prop<String>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    let top = signal(|| 0.0);
    let el = signal(|| None::<Rc<MountedData>>);
    let h = row_height.max(1.0);
    let at = memo(move || scroll_to.get());
    effect(move || {
        let (Some(i), Some(el)) = (at.get(), el.get_untracked()) else { return };
        spawn(async move { _ = el.scroll(PixelsVector2D::new(0.0, i as f64 * h), ScrollBehavior::Instant).await });
    });
    let rows = memo(move || window(len.get(), h, height.get(), overscan, top.get()));
    rsx! {
        div {
            class: join("aio-vlist", &class.get()),
            style: "overflow:auto;position:relative;height:{height}px",
            onmounted: move |e| *el.write() = Some(e.data()),
            // The browser tells a scroll once a frame at most; the rows
            // change only when that moves their window.
            onscroll: move |e| {
                let at = e.scroll_top();
                let (n, vh) = (len.get_untracked(), height.get_untracked());
                if window(n, h, vh, overscan, at) != window(n, h, vh, overscan, top.get_untracked()) {
                    top.set(at);
                }
            },
            ..attributes,
            div { style: "position:relative;height:{len.get() as f64 * h}px",
                for i in rows.get() {
                    div { key: i, style: "position:absolute;top:{i as f64 * h}px;left:0;right:0;height:{h}px",
                        {row.call(i)}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::window;

    #[test]
    fn the_window_is_the_rows_in_view_and_the_overscan() {
        // aio tests/virtual-list.test.ts.
        assert_eq!(window(100, 40.0, 200.0, 0, 0.0), 0..5);
        assert_eq!(window(100, 40.0, 200.0, 3, 0.0), 0..11, "overscan never goes below 0");
        assert_eq!(window(100, 40.0, 200.0, 2, 0.0), 0..9);
        assert_eq!(window(100, 40.0, 200.0, 0, 800.0), 20..25);
        assert_eq!(window(100, 40.0, 200.0, 0, 3800.0), 95..100);
        assert_eq!(window(10, 40.0, 200.0, 5, 0.0), 0..10, "nor past the end");
        assert_eq!(window(100, 40.0, 400.0, 0, 20.0), 0..11, "the cut bottom row is there too");
    }

    #[test]
    fn a_list_shorter_than_its_scroll_shows_its_last_window() {
        assert_eq!(window(10, 40.0, 200.0, 0, 4000.0), 5..10);
        assert_eq!(window(8, 40.0, 200.0, 0, 36_000.0), 3..8, "it shrank under a scrolled list");
        assert_eq!(window(1, 40.0, 200.0, 0, 36_000.0), 0..1);
        assert_eq!(window(0, 40.0, 200.0, 3, 36_000.0), 0..0);
    }

    #[test]
    fn a_row_is_at_least_a_pixel() {
        assert_eq!(window(5, 0.0, 200.0, 0, 0.0), 0..5);
        assert_eq!(window(3, -10.0, 100.0, 0, 0.0), 0..3);
    }
}

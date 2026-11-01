//! Enter and exit animations (aio's `Transition`, `TransitionGroup`): real
//! CSS keyframes, played forwards as content arrives and backwards as it
//! leaves, the leaving content kept until its exit is over. A group also
//! slides the rows that moved from where they were (FLIP). The list
//! bookkeeping is pure, for tests without a clock.
use super::join;
use dioxus::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

/// An animation: a `@keyframes` name (the kit's `aio-fade`, `aio-slide`,
/// `aio-scale`, or the app's own, written from the hidden state to the
/// shown one), how long, after what delay, with what easing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub keyframes: &'static str,
    pub ms: u32,
    pub delay_ms: u32,
    pub easing: &'static str,
}

impl Motion {
    /// Opacity 0 ↔ 1 (aio's `fade`; 300 ms, as all three).
    pub const FADE: Self = Self::new("aio-fade");
    /// Up from below its height, fading (aio's `slide`).
    pub const SLIDE: Self = Self::new("aio-slide");
    /// From nothing to full size, fading (aio's `scale`).
    pub const SCALE: Self = Self::new("aio-scale");

    pub const fn new(keyframes: &'static str) -> Self {
        Self { keyframes, ms: 300, delay_ms: 0, easing: "ease" }
    }

    pub const fn ms(self, ms: u32) -> Self {
        Self { ms, ..self }
    }

    /// How long until it is over.
    pub fn total(&self) -> Duration {
        Duration::from_millis((self.ms + self.delay_ms).into())
    }

    /// The `animation` declarations entering (`out`: leaving, backwards).
    pub fn css(&self, out: bool) -> String {
        let dir = if out { "reverse" } else { "normal" };
        let Self { keyframes, ms, delay_ms, easing } = self;
        format!("animation:{keyframes} {ms}ms {easing} {delay_ms}ms 1 {dir} both")
    }
}

/// Content that animates as `show` changes (aio's `Transition`): with no
/// `exit`, it goes at once. It sits in a box of its own (`class`, attributes).
#[component]
pub fn Transition(
    show: bool,
    enter: Option<Motion>,
    exit: Option<Motion>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: Element,
) -> Element {
    // (shown last render, leaving since run, runs so far): a show again cancels a leave's end.
    let state = use_hook(|| Rc::new(Cell::new((show, None::<u64>, 0u64))));
    let mut tick = use_signal(|| 0u64);
    _ = tick();
    let (was, mut leaving, mut runs) = state.get();
    if was
        && !show
        && let Some(m) = exit
    {
        runs += 1;
        leaving = Some(runs);
        let (state, run) = (state.clone(), runs);
        spawn(async move {
            super::sleep(m.total()).await;
            let (w, l, r) = state.get();
            if l == Some(run) {
                state.set((w, None, r));
                tick += 1;
            }
        });
    }
    if show {
        leaving = None;
    }
    state.set((show, leaving, runs));
    let out = leaving.is_some();
    if !show && !out {
        return rsx! {};
    }
    let style = if out { exit.map(|m| m.css(true)) } else { enter.map(|m| m.css(false)) };
    rsx! {
        div { class: join("aio-transition", &class), style, ..attributes, {children} }
    }
}

/// The rows to draw: `next` in its order, and each row of `before` that
/// is not in it (leaving, `true`) kept just after the row it followed.
pub fn merge_rows<K: PartialEq + Clone>(before: &[(K, bool)], next: &[K]) -> Vec<(K, bool)> {
    let mut out: Vec<(K, bool)> = next.iter().map(|k| (k.clone(), false)).collect();
    for (i, (k, _)) in before.iter().enumerate() {
        if next.contains(k) {
            continue;
        }
        let after = before[..i].iter().rev().find_map(|(p, _)| out.iter().position(|(o, _)| o == p));
        out.insert(after.map_or(0, |a| a + 1), (k.clone(), true));
    }
    out
}

/// How far each row moved between two layouts (FLIP's "invert"): rows
/// in both whose top-left changed, by key.
pub fn moved<K: Eq + std::hash::Hash + Clone>(
    before: &HashMap<K, (f64, f64)>,
    now: &HashMap<K, (f64, f64)>,
) -> HashMap<K, (f64, f64)> {
    now.iter()
        .filter_map(|(k, (x, y))| {
            let (bx, by) = before.get(k)?;
            let d = (bx - x, by - y);
            (d != (0.0, 0.0)).then(|| (k.clone(), d))
        })
        .collect()
}

/// One row's box and handle.
type Rows = Rc<RefCell<HashMap<String, (Element, Option<Rc<MountedData>>)>>>;

/// A keyed list whose rows animate as they come and go (aio
/// `TransitionGroup`); with `flip`, rows that moved slide from where they
/// were over `flip_ms`. Each row sits in a box of its own (`row_class`).
#[component]
pub fn TransitionGroup(
    items: Vec<(String, Element)>,
    enter: Option<Motion>,
    exit: Option<Motion>,
    #[props(default)] flip: bool,
    #[props(default = 300)] flip_ms: u32,
    #[props(default, into)] class: String,
    #[props(default, into)] row_class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> Element {
    let drawn = use_hook(|| Rc::new(RefCell::new(Vec::<(String, bool)>::new())));
    // Leaving rows whose exit is over.
    let gone = use_hook(|| Rc::new(RefCell::new(HashSet::<String>::new())));
    let rows: Rows = use_hook(Rows::default);
    // Where each row was last laid out, and the slide each one is making now.
    let at = use_hook(|| Rc::new(RefCell::new(HashMap::<String, (f64, f64)>::new())));
    let mut slide = use_signal(HashMap::<String, ((f64, f64), bool)>::new);
    let mut tick = use_signal(|| 0u64);
    _ = tick();
    let keys: Vec<String> = items.iter().map(|(k, _)| k.clone()).collect();
    let before = drawn.borrow().clone();
    let mut now = merge_rows(&before, &keys);
    {
        let mut g = gone.borrow_mut();
        // With no exit, a leaving row goes at once.
        now.retain(|(k, out)| !out || (exit.is_some() && !g.contains(k)));
        g.clear();
    }
    for (k, out) in &now {
        let was_out = before.iter().find(|(b, _)| b == k).map(|(_, o)| *o);
        if *out
            && was_out != Some(true)
            && let Some(m) = exit
        {
            let (k, gone) = (k.clone(), gone.clone());
            spawn(async move {
                super::sleep(m.total()).await;
                gone.borrow_mut().insert(k);
                tick += 1;
            });
        }
    }
    {
        let mut rs = rows.borrow_mut();
        for (k, el) in items {
            match rs.get_mut(&k) {
                Some(r) => r.0 = el,
                None => _ = rs.insert(k, (el, None)),
            }
        }
        rs.retain(|k, _| now.iter().any(|(n, _)| n == k));
    }
    if flip && now != before && !before.is_empty() {
        let (rows, at) = (rows.clone(), at.clone());
        spawn(async move {
            // Once this render is laid out: where each row is now.
            super::sleep(super::FRAME).await;
            let handles: Vec<_> =
                rows.borrow().iter().filter_map(|(k, (_, m))| Some((k.clone(), m.clone()?))).collect();
            let mut here = HashMap::new();
            for (k, m) in handles {
                if let Ok(r) = m.get_client_rect().await {
                    here.insert(k, (r.origin.x, r.origin.y));
                }
            }
            let d = moved(&at.borrow(), &here);
            *at.borrow_mut() = here;
            if d.is_empty() {
                return;
            }
            // Invert at once, then play back to where they are.
            slide.set(d.iter().map(|(k, v)| (k.clone(), (*v, false))).collect());
            super::sleep(super::FRAME).await;
            slide.set(d.into_iter().map(|(k, v)| (k, (v, true))).collect());
            super::sleep(Duration::from_millis(flip_ms.into())).await;
            slide.set(HashMap::new());
        });
    }
    *drawn.borrow_mut() = now.clone();
    let shown: Vec<(String, bool, Element)> = {
        let rs = rows.borrow();
        now.iter().filter_map(|(k, out)| Some((k.clone(), *out, rs.get(k)?.0.clone()))).collect()
    };
    let slides = slide();
    rsx! {
        div { class: join("aio-tgroup", &class), ..attributes,
            for (k, out, el) in shown {
                div {
                    key: "{k}",
                    class: join("aio-tgroup__row", &row_class),
                    style: row_style(if out { exit.map(|m| m.css(true)) } else { enter.map(|m| m.css(false)) }, slides.get(&k), flip_ms),
                    onmounted: {
                        let (rows, at, k) = (rows.clone(), at.clone(), k.clone());
                        move |e: MountedEvent| {
                            let m = e.data();
                            if let Some(r) = rows.borrow_mut().get_mut(&k) {
                                r.1 = Some(m.clone());
                            }
                            let (at, k) = (at.clone(), k.clone());
                            async move {
                                if let Ok(r) = m.get_client_rect().await {
                                    at.borrow_mut().entry(k).or_insert((r.origin.x, r.origin.y));
                                }
                            }
                        }
                    },
                    {el}
                }
            }
        }
    }
}

/// A row's style: its enter or exit animation, and the slide it is making
/// (inverted at once, then played back with a transition).
fn row_style(anim: Option<String>, slide: Option<&((f64, f64), bool)>, ms: u32) -> String {
    let slide = match slide {
        Some(((dx, dy), false)) => format!("transform:translate({dx}px,{dy}px);transition:none"),
        Some((_, true)) => format!("transform:none;transition:transform {ms}ms ease"),
        None => String::new(),
    };
    [anim.unwrap_or_default(), slide].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(s: &[(&str, bool)]) -> Vec<(String, bool)> {
        s.iter().map(|(k, o)| (k.to_string(), *o)).collect()
    }

    fn keys(s: &[&str]) -> Vec<String> {
        s.iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn a_leaving_row_stays_where_it_was_until_it_is_gone() {
        let before = rows(&[("a", false), ("b", false), ("c", false)]);
        assert_eq!(merge_rows(&before, &keys(&["a", "c"])), rows(&[("a", false), ("b", true), ("c", false)]));
        assert_eq!(
            merge_rows(&before, &keys(&["c", "d"])),
            rows(&[("a", true), ("b", true), ("c", false), ("d", false)])
        );
        assert_eq!(merge_rows(&before, &[]), rows(&[("a", true), ("b", true), ("c", true)]), "all at once");
        let leaving = rows(&[("a", false), ("b", true)]);
        assert_eq!(merge_rows(&leaving, &keys(&["b", "a"])), rows(&[("b", false), ("a", false)]), "back again");
        assert!(merge_rows::<String>(&[], &[]).is_empty());
    }

    #[test]
    fn a_row_moved_by_the_difference_and_new_or_still_rows_not_at_all() {
        let before = HashMap::from([("a", (0.0, 0.0)), ("b", (0.0, 20.0)), ("c", (0.0, 40.0))]);
        let now = HashMap::from([("a", (0.0, 40.0)), ("b", (0.0, 20.0)), ("d", (0.0, 0.0))]);
        assert_eq!(moved(&before, &now), HashMap::from([("a", (0.0, -40.0))]));
    }

    #[test]
    fn a_motion_plays_forwards_in_and_backwards_out() {
        let m = Motion { delay_ms: 50, ..Motion::FADE.ms(200) };
        assert_eq!(m.css(false), "animation:aio-fade 200ms ease 50ms 1 normal both");
        assert_eq!(m.css(true), "animation:aio-fade 200ms ease 50ms 1 reverse both");
        assert_eq!(m.total(), Duration::from_millis(250));
        assert_eq!(
            row_style(None, Some(&((0.0, -40.0), false)), 300),
            "transform:translate(0px,-40px);transition:none"
        );
    }
}

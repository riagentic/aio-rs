//! Enter and exit animations (aio's `Transition`, `TransitionGroup`): real
//! CSS keyframes, played forwards as content arrives and backwards as it
//! leaves, the leaving content kept until its exit is over. A group also
//! slides the rows that moved from where they were (FLIP). The list
//! bookkeeping is pure, for tests without a clock.
use super::join;
use crate::ui::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

/// An animation: a `@keyframes` name (the kit's `aio-fade`, `aio-slide`,
/// `aio-scale`, or the app's own, written from the hidden state to the
/// shown one), how long, after what delay, with what easing. Or a
/// function of the progress ([`Motion::of`], aio's `TransitionFn`).
#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub keyframes: &'static str,
    pub ms: u32,
    pub delay_ms: u32,
    pub easing: &'static str,
    /// The declarations at progress `t` (0: hidden, 1: shown), sampled into `keyframes`.
    pub css: Option<fn(f64) -> String>,
    /// Called each frame while it plays with `(t, u)`, `u = 1 - t` (aio's
    /// `TransitionFn` `tick`): what CSS can't do (a typed-out text, a counter).
    pub tick: Option<fn(f64, f64)>,
}

impl PartialEq for Motion {
    /// The same animation: its name stands for its frames.
    fn eq(&self, o: &Self) -> bool {
        (self.keyframes, self.ms, self.delay_ms, self.easing) == (o.keyframes, o.ms, o.delay_ms, o.easing)
    }
}

/// One duration, delay and easing for both of a transition's motions
/// (aio's `options` prop): each field given replaces the motion's own. A
/// `const` of it is the defaults many transitions share.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Timing {
    pub ms: Option<u32>,
    pub delay_ms: Option<u32>,
    pub easing: Option<&'static str>,
}

/// How many steps a [`Motion::of`] is sampled in (aio's `KEYFRAME_STEPS`).
const STEPS: u32 = 20;

impl Motion {
    /// Opacity 0 ↔ 1 (aio's `fade`; 300 ms, as all three).
    pub const FADE: Self = Self::new("aio-fade");
    /// Up from below its height, fading (aio's `slide`).
    pub const SLIDE: Self = Self::new("aio-slide");
    /// From nothing to full size, fading (aio's `scale`).
    pub const SCALE: Self = Self::new("aio-scale");

    pub const fn new(keyframes: &'static str) -> Self {
        Self { keyframes, ms: 300, delay_ms: 0, easing: "ease", css: None, tick: None }
    }

    /// A motion written as a function (aio's `TransitionFn` `css(t)`):
    /// `css(t)` is the style at progress `t`, 0 hidden to 1 shown
    /// (`|t| format!("opacity:{t};transform:scale({t})")`). `name` names
    /// its `@keyframes`: one name, one function. Where aio's function was
    /// given the element, the frames read its box: `var(--aio-w)` and
    /// `var(--aio-h)`, set on it (measured without the animation) before its
    /// first frame.
    pub const fn of(name: &'static str, css: fn(f64) -> String) -> Self {
        Self { css: Some(css), ..Self::new(name) }
    }

    /// The `@keyframes` rule a [`Motion::of`] plays (none for a named one).
    pub fn rule(&self) -> Option<String> {
        let css = self.css?;
        let frames: String = (0..=STEPS)
            .map(|i| {
                let t = f64::from(i) / f64::from(STEPS);
                format!("{:.1}%{{{}}}", t * 100.0, css(t))
            })
            .collect();
        Some(format!("@keyframes {}{{{frames}}}", self.keyframes))
    }

    /// Put its rule in the page, once (a named motion's is the app's or the kit's).
    fn install(&self) {
        let Some(rule) = self.rule() else { return };
        let id = serde_json::to_string(&format!("aio-motion-{}", self.keyframes)).unwrap_or_default();
        let rule = serde_json::to_string(&rule).unwrap_or_default();
        ::air::exec(&format!(
            "if(!document.getElementById({id})){{const s=document.createElement('style');s.id={id};\
             s.textContent={rule};document.head.append(s)}}"
        ));
    }

    pub const fn ms(self, ms: u32) -> Self {
        Self { ms, ..self }
    }

    /// With `t`'s fields where it gives them.
    pub fn timed(self, t: Timing) -> Self {
        Self {
            ms: t.ms.unwrap_or(self.ms),
            delay_ms: t.delay_ms.unwrap_or(self.delay_ms),
            easing: t.easing.unwrap_or(self.easing),
            ..self
        }
    }

    /// `tick(t, u)` each frame while it plays, entering 0 → 1, leaving 1 → 0
    /// (the last call at the end exactly).
    pub const fn ticking(self, tick: fn(f64, f64)) -> Self {
        Self { tick: Some(tick), ..self }
    }

    /// Run its `tick` over its time, toward `shown`, while `run` is the latest.
    async fn ticks(self, shown: bool, runs: Rc<Cell<u64>>, run: u64) {
        let Some(tick) = self.tick else { return };
        super::sleep(Duration::from_millis(self.delay_ms.into())).await;
        let (start, ms) = (super::Instant::now(), f64::from(self.ms.max(1)));
        while runs.get() == run {
            let p = ((super::Instant::now() - start).as_secs_f64() * 1000.0 / ms).min(1.0);
            let t = if shown { p } else { 1.0 - p };
            tick(t, 1.0 - t);
            if p >= 1.0 {
                return;
            }
            super::sleep(Duration::from_millis(16)).await;
        }
    }

    /// How long until it is over.
    pub fn total(&self) -> Duration {
        Duration::from_millis((self.ms + self.delay_ms).into())
    }

    /// The `animation` declaration entering (`out`: leaving, backwards).
    pub fn css(&self, out: bool) -> String {
        format!("animation:{}", self.animation(out))
    }

    /// The `animation` value entering (`out`: leaving, backwards).
    pub fn animation(&self, out: bool) -> String {
        let dir = if out { "reverse" } else { "normal" };
        let Self { keyframes, ms, delay_ms, easing, .. } = self;
        format!("{keyframes} {ms}ms {easing} {delay_ms}ms 1 {dir} both")
    }
}

/// `enter` and `exit` with `options` over each (pure).
fn both(enter: Option<Motion>, exit: Option<Motion>, options: Option<Timing>) -> (Option<Motion>, Option<Motion>) {
    let t = |m: Option<Motion>| m.map(|m| options.map_or(m, |o| m.timed(o)));
    (t(enter), t(exit))
}

/// Content that animates as `show` changes (aio's `Transition`): with no
/// `exit`, it goes at once. No box of its own: its content's first element
/// plays the animation (its own style kept). `options` times both motions.
#[component]
pub fn Transition(
    show: Prop<bool>,
    enter: Option<Motion>,
    exit: Option<Motion>,
    options: Option<Timing>,
    children: View,
) -> View {
    let (enter, exit) = both(enter, exit, options);
    once(|| enter.iter().chain(&exit).for_each(Motion::install));
    let on = signal(|| show.get_untracked());
    let leaving = signal(|| false);
    // A show again cancels a leave's end.
    let runs = once(|| Rc::new(Cell::new(0u64)));
    let was = once(|| Rc::new(Cell::new(show.get_untracked())));
    effect(move || {
        let now = show.get();
        if now == was.replace(now) {
            return;
        }
        runs.set(runs.get() + 1);
        if now {
            leaving.set(false);
            on.set(true);
            if let Some(m) = enter {
                spawn(m.ticks(true, runs.clone(), runs.get()));
            }
            return;
        }
        let Some(m) = exit else { return on.set(false) };
        leaving.set(true);
        spawn(m.ticks(false, runs.clone(), runs.get()));
        let (runs, run) = (runs.clone(), runs.get());
        spawn(async move {
            super::sleep(m.total()).await;
            if runs.get() == run {
                leaving.set(false);
                on.set(false);
            }
        });
    });
    let content = ::air::styled(children, "animation", move || {
        if leaving.get() { exit.map(|m| m.animation(true)) } else { enter.map(|m| m.animation(false)) }
    });
    rsx! {
        if on.get() {
            {content.clone()}
        }
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

/// A keyed list whose rows animate as they come and go (aio
/// `TransitionGroup`); with `flip`, rows that moved slide from where they
/// were over `flip_ms`. Each row sits in a box of its own (`row_class`).
/// `options` times both motions.
#[component]
pub fn TransitionGroup(
    #[props(into)] items: Prop<Vec<(String, View)>>,
    enter: Option<Motion>,
    exit: Option<Motion>,
    options: Option<Timing>,
    #[props(default)] flip: bool,
    #[props(default = 300)] flip_ms: u32,
    #[props(default, into)] class: Prop<String>,
    #[props(default, into)] row_class: Prop<String>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    let (enter, exit) = both(enter, exit, options);
    once(|| enter.iter().chain(&exit).for_each(Motion::install));
    // The rows drawn (key, leaving), each row's view and handle.
    let drawn = signal(Vec::<(String, bool)>::new);
    let views = once(|| Rc::new(RefCell::new(HashMap::<String, View>::new())));
    let handles = once(|| Rc::new(RefCell::new(HashMap::<String, Rc<MountedData>>::new())));
    // Where each row was last laid out, and the slide each one is making now.
    let at = once(|| Rc::new(RefCell::new(HashMap::<String, (f64, f64)>::new())));
    let slide = signal(HashMap::<String, ((f64, f64), bool)>::new);
    // Leaving rows whose exit is over, and a tick to take them out.
    let gone = signal(HashSet::<String>::new);
    // The group's timers live as long as it, not as one run of the effect
    // (a change while a row leaves must not cancel its exit).
    let me = ::air::current_owner();
    let later = move |f: std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>| match me {
        Some(o) => _ = ::air::with_owner(o, || spawn(f)),
        None => _ = spawn(f),
    };
    {
        let (views, handles, at) = (views.clone(), handles.clone(), at.clone());
        effect(move || {
            let next = items.get();
            gone.with(|_| ());
            let keys: Vec<String> = next.iter().map(|(k, _)| k.clone()).collect();
            let before = drawn.get_untracked();
            let mut now = merge_rows(&before, &keys);
            let done = gone.get_untracked();
            // With no exit, a leaving row goes at once.
            now.retain(|(k, out)| !out || (exit.is_some() && !done.contains(k)));
            for (k, out) in &now {
                let was_out = before.iter().find(|(b, _)| b == k).map(|(_, o)| *o);
                if *out
                    && was_out != Some(true)
                    && let Some(m) = exit
                {
                    let k = k.clone();
                    later(Box::pin(async move {
                        super::sleep(m.total()).await;
                        gone.update(|g| _ = g.insert(k));
                    }));
                }
            }
            {
                let mut vs = views.borrow_mut();
                for (k, v) in next {
                    vs.insert(k, v);
                }
                vs.retain(|k, _| now.iter().any(|(n, _)| n == k));
                handles.borrow_mut().retain(|k, _| now.iter().any(|(n, _)| n == k));
            }
            if done.iter().any(|k| !now.iter().any(|(n, _)| n == k)) {
                gone.update(|g| g.retain(|k| now.iter().any(|(n, _)| n == k)));
            }
            if flip && now != before && !before.is_empty() {
                let (handles, at) = (handles.clone(), at.clone());
                later(Box::pin(async move {
                    // Once this change is laid out: where each row is now.
                    super::sleep(super::FRAME).await;
                    let hs: Vec<_> = handles.borrow().iter().map(|(k, m)| (k.clone(), m.clone())).collect();
                    let mut here = HashMap::new();
                    for (k, m) in hs {
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
                }));
            }
            drawn.set(now);
        });
    }
    let keys = memo(move || drawn.with(|d| d.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>()));
    rsx! {
        div { class: join("aio-tgroup", &class.get()), ..attributes,
            for k in keys.get() {
                div {
                    key: "{k}",
                    class: join("aio-tgroup__row", &row_class.get()),
                    style: {
                        let out = drawn.with(|d| d.iter().any(|(r, o)| *r == k && *o));
                        row_style(if out { exit.map(|m| m.css(true)) } else { enter.map(|m| m.css(false)) }, slide.with(|s| s.get(&k).copied()).as_ref(), flip_ms)
                    },
                    onmounted: {
                        let (handles, at, k) = (handles.clone(), at.clone(), k.clone());
                        move |e: MountedEvent| {
                            let m = e.data();
                            handles.borrow_mut().insert(k.clone(), m.clone());
                            let (at, k) = (at.clone(), k.clone());
                            async move {
                                if let Ok(r) = m.get_client_rect().await {
                                    at.borrow_mut().entry(k).or_insert((r.origin.x, r.origin.y));
                                }
                            }
                        }
                    },
                    {views.borrow().get(&k).cloned()}
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
        let grow = Motion::of("grow", |t| format!("opacity:{t}"));
        let rule = grow.rule().unwrap();
        assert!(rule.starts_with("@keyframes grow{0.0%{opacity:0}5.0%{opacity:0.05}"), "{rule}");
        assert!(rule.ends_with("100.0%{opacity:1}}"), "{rule}");
        assert_eq!(
            (Motion::FADE.rule(), grow, grow.css(false)),
            (None, Motion::of("grow", |_| String::new()), "animation:grow 300ms ease 0ms 1 normal both".into())
        );
        const QUICK: Timing = Timing { ms: Some(120), delay_ms: None, easing: Some("ease-out") };
        let (e, x) = both(Some(Motion::FADE), Some(m), Some(QUICK));
        assert_eq!(e.unwrap().css(false), "animation:aio-fade 120ms ease-out 0ms 1 normal both");
        assert_eq!(x.unwrap().css(true), "animation:aio-fade 120ms ease-out 50ms 1 reverse both", "its own delay kept");
        assert_eq!(both(None, Some(m), None), (None, Some(m)), "no options: as given");
        assert_eq!(
            row_style(None, Some(&((0.0, -40.0), false)), 300),
            "transform:translate(0px,-40px);transition:none"
        );
    }
}

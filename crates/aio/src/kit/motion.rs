//! Motion (aio's `useSpring`, `useInterval`, `useRaf`): a number that springs
//! to where it is sent, and loops that tick while their component lives
//! and `active` holds. A tick that panics is logged (the first, then each
//! tenfold count) and the loop goes on: one bad frame never freezes a view.
//! The physics is pure, for tests without a clock.
use super::Instant;
use crate::ui::prelude::*;
use Task;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// One frame at 60 Hz.
pub const FRAME: Duration = Duration::from_micros(16_667);

/// Spring physics (aio's defaults: stiffness 170, damping 26, mass 1,
/// precision 0.01).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub stiffness: f64,
    pub damping: f64,
    pub mass: f64,
    /// Closer than this to the target, and slower, is at rest.
    pub precision: f64,
}

impl Default for Spring {
    fn default() -> Self {
        Self { stiffness: 170.0, damping: 26.0, mass: 1.0, precision: 0.01 }
    }
}

/// One `dt`-second step of position `x` moving at `v` toward `target`: the
/// new position and velocity, or `None` once at rest (then it is at
/// `target`). A massless spring is at rest at once.
pub fn spring_step(s: Spring, (x, v): (f64, f64), target: f64, dt: f64) -> Option<(f64, f64)> {
    if s.mass <= 0.0 {
        return None;
    }
    let a = (-s.stiffness * (x - target) - s.damping * v) / s.mass;
    let v = v + a * dt;
    let x = x + v * dt;
    (v.abs() >= s.precision || (x - target).abs() >= s.precision).then_some((x, v))
}

/// A sprung number ([`use_spring`]): read `value` in a view, send it with `to`.
#[derive(Clone, Copy, PartialEq)]
pub struct SpringValue {
    value: Signal<f64>,
    target: Signal<f64>,
    velocity: Signal<f64>,
    moving: Signal<bool>,
    /// Bumped by every `set` and new run, so one loop runs at a time.
    run: Signal<u64>,
    spring: Spring,
}

impl SpringValue {
    /// Where it is now (reading subscribes the view).
    pub fn value(&self) -> f64 {
        (self.value).get()
    }

    /// Is it still moving?
    pub fn animating(&self) -> bool {
        (self.moving).get()
    }

    /// Spring to `target`, from where it is and how fast it moves.
    pub fn to(self, target: f64) {
        self.target.set(target);
        if *self.moving.read_untracked() {
            return;
        }
        self.moving.set(true);
        let run = *self.run.read_untracked() + 1;
        self.run.set(run);
        spawn(async move {
            loop {
                super::sleep(FRAME).await;
                if *self.run.read_untracked() != run {
                    return;
                }
                let (now, target) =
                    ((*self.value.read_untracked(), *self.velocity.read_untracked()), *self.target.read_untracked());
                match spring_step(self.spring, now, target, FRAME.as_secs_f64()) {
                    Some((x, v)) => {
                        self.value.set(x);
                        self.velocity.set(v);
                    }
                    None => return self.set(target),
                }
            }
        });
    }

    /// Be at `value` now, at rest.
    pub fn set(mut self, value: f64) {
        self.run += 1;
        self.target.set(value);
        self.velocity.set(0.0);
        self.value.set(value);
        self.moving.set(false);
    }
}

/// A number that springs (a hook): starts at `initial`, at rest. Its loop
/// stops with the component (no `dispose` to forget).
pub fn use_spring(initial: f64, spring: Spring) -> SpringValue {
    let value = signal(|| initial);
    let target = signal(|| initial);
    let velocity = signal(|| 0.0);
    let moving = signal(|| false);
    let run = signal(|| 0);
    SpringValue { value, target, velocity, moving, run, spring }
}

/// Run `tick` and contain a panic: logged at the first and each tenfold
/// count (`which` names the loop), so a tick that fails every frame is
/// said, not flooded.
fn guarded(which: &str, fails: &Cell<u64>, tick: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(tick)).is_err() {
        let n = fails.get() + 1;
        fails.set(n);
        if n == 10u64.pow(n.ilog10()) {
            crate::error!(cat: "ui", "{which}: the tick panicked ({n} so far); the loop goes on");
        }
    }
}

/// A loop task per component, (re)started while `active` holds and
/// stopped otherwise (and with the component).
fn use_loop(active: Prop<bool>, start: impl Fn() -> Task + 'static) {
    let task = once(|| Rc::new(Cell::new(None::<Task>)));
    effect(move || match (active.get(), task.get()) {
        (true, None) => task.set(Some(start())),
        (false, Some(t)) => {
            t.cancel();
            task.set(None);
        }
        _ => {}
    });
}

/// Call `tick` every `every` while `active` (aio's `useInterval`; a
/// signal or a memo is followed, a `bool` is fixed).
pub fn use_interval(every: Duration, active: impl Into<Prop<bool>>, mut tick: impl FnMut() + 'static) {
    let tick = callback(move |()| tick());
    use_loop(active.into(), move || {
        spawn(async move {
            let fails = Cell::new(0);
            loop {
                super::sleep(every).await;
                guarded("use_interval", &fails, || tick.call(()));
            }
        })
    });
}

/// Call `frame(since_start, since_last)` once a frame (60 Hz) while
/// `active` (aio's `useRaf`; the first frame's `since_last` is zero). For
/// state that moves; a canvas drawn per frame belongs in the page.
pub fn use_frame(active: impl Into<Prop<bool>>, mut frame: impl FnMut(Duration, Duration) + 'static) {
    let frame = callback(move |(t, dt)| frame(t, dt));
    use_loop(active.into(), move || {
        spawn(async move {
            let (start, fails) = (Instant::now(), Cell::new(0));
            let mut last = None;
            loop {
                let now = Instant::now();
                let dt = last.map_or(Duration::ZERO, |l| now - l);
                last = Some(now);
                guarded("use_frame", &fails, || frame.call((now - start, dt)));
                super::sleep(FRAME).await;
            }
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steps from `x` at rest to `target` (`None`: not at rest in 10 s).
    fn settle(s: Spring, x: f64, target: f64) -> Option<(usize, Vec<f64>)> {
        let (mut at, mut path) = ((x, 0.0), vec![]);
        for n in 0..600 {
            match spring_step(s, at, target, FRAME.as_secs_f64()) {
                Some(next) => {
                    at = next;
                    path.push(next.0);
                }
                None => return Some((n, path)),
            }
        }
        None
    }

    #[test]
    fn the_default_spring_comes_to_rest_at_its_target_without_overshoot() {
        let (frames, path) = settle(Spring::default(), 0.0, 100.0).expect("at rest");
        assert!((20..120).contains(&frames), "{frames} frames");
        assert!(path.windows(2).all(|w| w[1] >= w[0] - 1e-9), "critically damped: never back");
        assert!(path.iter().all(|&x| x <= 100.0 + 0.5), "no real overshoot");
    }

    #[test]
    fn a_loose_spring_overshoots_and_still_settles() {
        let s = Spring { damping: 5.0, ..Spring::default() };
        let (_, path) = settle(s, 0.0, 10.0).expect("at rest");
        assert!(path.iter().any(|&x| x > 10.0), "it swings past");
    }

    #[test]
    fn at_its_target_or_massless_a_spring_is_at_rest() {
        assert_eq!(spring_step(Spring::default(), (5.0, 0.0), 5.0, 0.016), None);
        assert_eq!(spring_step(Spring { mass: 0.0, ..Spring::default() }, (0.0, 0.0), 100.0, 0.016), None);
    }

    #[test]
    fn a_panicking_tick_is_said_at_tenfold_counts_and_contained() {
        let fails = Cell::new(0);
        for _ in 0..12 {
            guarded("t", &fails, || panic!("bad frame"));
        }
        guarded("t", &fails, || {});
        assert_eq!(fails.get(), 12, "a good tick isn't counted");
    }
}

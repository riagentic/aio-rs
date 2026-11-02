# Motion and animation

Motion in aio-rs is CSS first. `aio::kit` (feature `kit`; aio's AIR
animation) has three tools:

| Tool | For | aio |
|---|---|---|
| `Transition` | one piece of content that comes and goes (a panel, a banner) | `<Transition>` |
| `TransitionGroup` | a keyed list whose rows come, go and move (FLIP) | `<TransitionGroup>` |
| `use_spring` | a number that moves smoothly to where it is sent (a drawer's offset, a gauge) | `useSpring` |

plus two loops, `use_frame` (once a frame) and `use_interval` (on a timer),
for state that changes over time.

- **Use CSS alone** (`transition: background .2s`) for hover and focus
  states: no Rust needed.
- **Use `Transition`** when content leaves the page: CSS can't animate an
  element that is already gone; `Transition` keeps it until its exit has
  played.
- **Use `use_spring`** when the end point changes while moving (a drag let
  go, a value that keeps arriving): a spring carries its speed into the new
  target, a CSS transition starts over.

## Enter and exit: `Transition`

```rust,no_run
use aio::kit::{Motion, Transition};
use aio::ui::prelude::*;

#[component]
fn Notice() -> View {
    let show = signal(|| true);
    rsx! {
        button { "data-t": "toggle", onclick: move |_| show.toggle(), "Toggle" }
        Transition { show: show.get(), enter: Motion::FADE, exit: Motion::FADE.ms(100),
            p { "data-t": "msg", "Saved" }
        }
    }
}

# fn main() {
use std::time::Duration;
let mut ui = aio::testing::TestUi::new(Notice);
assert!(ui.attr("msg", "style").unwrap().contains("aio-fade"), "its first element plays it");
ui.click("toggle");
assert!(ui.present("msg"), "kept while its exit plays");
ui.wait(Duration::from_millis(150));
assert!(ui.absent("msg"), "gone once it is over");
# }
```

| Prop | Type | Does |
|---|---|---|
| `show` | `Prop<bool>` | whether the content is there (an expression follows: `show: open.get()`) |
| `enter` | `Option<Motion>` | played as it arrives (none: it appears at once) |
| `exit` | `Option<Motion>` | played backwards as it leaves; the content stays until it is over (none: it goes at once) |

- No box of its own: the content's first element plays the animation
  (its `animation` style set, the rest of its style kept), so give the
  content one element at its root.
- Show, hide, show again quickly: the leave is cancelled and nothing is
  removed.
- Unlike aio, the content is given as children with `show`, not as a
  conditional child.

## Motions

A `Motion` names CSS `@keyframes` written from the hidden state to the
shown one. Entering plays them forwards, leaving plays them in reverse,
so one keyframe set does both.

| Motion | Keyframes (`from`) | aio |
|---|---|---|
| `Motion::FADE` | `opacity: 0` | `fade` |
| `Motion::SLIDE` | `opacity: 0; transform: translateY(100%)` | `slide` |
| `Motion::SCALE` | `opacity: 0; transform: scale(0)` | `scale` |

Each lasts 300 ms with `ease`, no delay. `SLIDE` travels the element's
own height and `SCALE` grows from nothing, as aio's presets do; write your
own keyframes for something gentler.

| Field or call | Default | Does |
|---|---|---|
| `Motion::new("my-keyframes")` | | your own `@keyframes`, in the app's CSS |
| `.ms(200)` | 300 | how long |
| `delay_ms` | 0 | wait before playing (`Motion { delay_ms: 50, ..Motion::FADE }`) |
| `easing` | `"ease"` | any CSS timing function (`Motion { easing: "ease-out", ..m }`) |
| `m.css(out)` | | the `animation` declaration it writes (`out`: reversed) |
| `m.total()` | | `ms + delay_ms`, how long until it is over |

```rust
use aio::kit::Motion;

/// In the app's CSS: `@keyframes rise { from { opacity: 0; transform: translateY(8px) } }`
const RISE: Motion = Motion { easing: "ease-out", ..Motion::new("rise").ms(180) };

# fn main() {
assert_eq!(RISE.css(false), "animation:rise 180ms ease-out 0ms 1 normal both");
assert_eq!(RISE.css(true), "animation:rise 180ms ease-out 0ms 1 reverse both");
assert_eq!(RISE.total().as_millis(), 180);
# }
```

## Lists: `TransitionGroup`

```rust,no_run
use aio::kit::{Motion, TransitionGroup};
use aio::ui::prelude::*;

#[component]
fn Queue() -> View {
    let items = signal(|| vec!["milk", "eggs", "bread"]);
    // `items` is a `Prop`: given an expression, it follows what that reads.
    let rows = move || -> Vec<(String, View)> {
        items.get().into_iter().map(|k| (k.to_string(), rsx! { p { "data-t": "{k}", "{k}" } })).collect()
    };
    rsx! {
        button { "data-t": "drop", onclick: move |_| items.set(vec!["milk", "bread", "tea"]), "Drop eggs" }
        TransitionGroup { items: rows(), enter: Motion::FADE, exit: Motion::FADE.ms(200), flip: true, flip_ms: 150 }
    }
}

# fn main() {
use std::time::Duration;
let mut ui = aio::testing::TestUi::new(Queue);
ui.click("drop");
assert!(ui.present("eggs"), "it leaves where it was");
ui.wait(Duration::from_millis(400));
assert!(ui.absent("eggs") && ui.present("tea"));
# }
```

| Prop | Default | Does |
|---|---|---|
| `items` | | the rows, `Prop<Vec<(key, View)>>`, in order; the key says which row is which |
| `enter`, `exit` | none | as `Transition`'s, per row |
| `flip` | `false` | rows that moved slide from where they were |
| `flip_ms` | 300 | how long that slide takes |
| `class` | | on the group's box (`div.aio-tgroup`) |
| `row_class` | | on each row's box (`div.aio-tgroup__row`) |

- A row that leaves stays in its place, just after the row it followed,
  until its exit is over; a row that comes back meanwhile stays.
- FLIP: after a change is laid out, each row's new place is measured
  against its old one, the row is moved back there at once, then let go
  with a `transform` transition. Rows that did not move are left alone.
- Keys must be stable (an id, not the index): a key is how a row is
  followed across changes.
- `merge_rows(before, next)` and `moved(before, now)` are the pure parts
  (which rows to draw, how far each moved), for your own list.

## Springs: `use_spring`

```rust,no_run
use aio::kit::{Spring, use_spring};
use aio::ui::prelude::*;

#[component]
fn Drawer() -> View {
    let x = use_spring(0.0, Spring::default());
    rsx! {
        button { "data-t": "open", onclick: move |_| x.to(if x.value() > 0.0 { 0.0 } else { 240.0 }), "Menu" }
        nav { "data-t": "drawer", style: "transform: translateX({x.value()}px)", "Links" }
        p { "data-t": "x", "{x.value():.0} {x.animating()}" }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Drawer);
ui.click("open");
ui.until_text("x", "240 false");
# }
```

`use_spring(initial, spring)` returns a `SpringValue` (`Copy`):

| Call | Does |
|---|---|
| `x.value()` | where it is now; a hole reading it runs again each frame it moves |
| `x.animating()` | still moving |
| `x.to(target)` | spring there, from where it is and as fast as it moves now |
| `x.set(value)` | be there now, at rest |

| `Spring` field | Default | Does |
|---|---|---|
| `stiffness` | 170 | how hard it pulls toward the target |
| `damping` | 26 | how much it brakes; low values swing past and back |
| `mass` | 1 | how heavy; 0 or less is at rest at once |
| `precision` | 0.01 | closer and slower than this is at rest (then it is exactly at the target) |

The defaults are aio's, critically damped: no overshoot. The loop runs at
60 Hz (`aio::kit::FRAME`) and stops with its component: there is no
`dispose` to forget. `aio::kit::spring_step(spring, (x, v), target, dt)` is
the physics alone, one step, for tests and for your own loop:

```rust
use aio::kit::{FRAME, Spring, spring_step};

# fn main() {
let (mut at, mut frames) = ((0.0, 0.0), 0);
while let Some(next) = spring_step(Spring::default(), at, 100.0, FRAME.as_secs_f64()) {
    at = next;
    frames += 1;
}
assert!(frames > 10 && frames < 120, "at rest in under two seconds");
# }
```

## Loops: `use_frame`, `use_interval`

| Hook | Calls | While |
|---|---|---|
| `use_frame(active, \|since_start, since_last\| ..)` | once a frame (60 Hz); the first `since_last` is zero | `active` holds and the component lives (aio's `useRaf`) |
| `use_interval(every, active, \|\| ..)` | every `every` | the same (aio's `useInterval`) |

Each always calls the latest closure, restarts when `active` turns true
again, and stops when it turns false or the component goes. A tick that
panics is logged (the first time, then at 10, 100…) and the loop goes on.
They run in the view, so they move view state; a canvas drawn every frame
belongs in an island ([islands](islands.md)), and data that changes on a
clock belongs in a cell's `#[every]` ([effects](../state/effects.md#schedules)).

## Reduced motion

The kit's stylesheet honours `prefers-reduced-motion: reduce`: transitions
and group rows play in 1 ms, the kit's toasts and skeletons stop moving,
and its spinner turns slower. A spring is not CSS: check the setting yourself when a
spring's movement is decoration (a media query in your CSS, or a setting
in a cell).

## Testing motion

`TestUi` runs the real timers, so a test sees each phase:

- `ui.attr(t, "style")` holds the `animation:` a `Transition` wrote
  (`normal` entering, `reverse` leaving).
- `ui.wait(d)` lets an exit finish; `ui.until_text(t, want)` waits for a
  spring to come to rest.
- The pure parts (`spring_step`, `merge_rows`, `moved`, `Motion::css`)
  test without a page at all.

## Common mistakes

- `if open() { Transition { .. } }`: the `Transition` itself goes, so it
  can't play an exit. Keep it in the tree and pass `show`.
- A `TransitionGroup` keyed by index: removing one row looks like the last
  row leaving and the others changing. Key by id.
- A keyframe set written from shown to hidden: aio-rs plays `from` as the
  hidden state. Write `from { opacity: 0 }`, not `to { opacity: 0 }`.
- Reading `x.value()` in a view's body: read once, it never moves. Read
  it in a hole (`style: "opacity: {x.value()}"`), as close to the moving
  element as it can be.

## Gaps

- aio's transitions took a `TransitionFn` (a `css(t)` function of
  progress); aio-rs takes CSS `@keyframes` by name, or `Motion::of("grow",
  |t| format!("opacity:{t};transform:scale({t})"))`, that function
  sampled into `@keyframes grow` (20 steps, as aio's) and put in the page
  once.
- aio's `options` prop on `Transition` and `TransitionGroup` set one
  duration, delay and easing for both; in aio-rs each `Motion` carries its
  own.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::kit::Transition`,
`TransitionGroup`, `Motion`, `merge_rows`, `moved`, `use_spring`,
`SpringValue`, `Spring`, `spring_step`, `FRAME`, `use_frame`,
`use_interval`.

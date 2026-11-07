//! The app: a world of bouncing balls (its cell, and `step`, the pure
//! function that moves it) and its window, a canvas. Every decision is in
//! `step`, tested without a GPU; only the drawing (one script a frame) is
//! not, and a screenshot covers it (`ax shot --check`). `ax dev` hot-patches
//! all of this library.
use aio::ui::prelude::*;

/// One body: where it is and how it moves, a frame at a time.
#[aio::data]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ball {
    pub x: f64,
    pub y: f64,
    pub dx: f64,
    pub dy: f64,
}

/// The world: kept while the app runs, not saved (a frame's moves are
/// drawing, not history); `ax state <app> world` shows it.
#[aio::cell(custom_default, id = "world")]
pub struct World {
    pub width: f64,
    pub height: f64,
    pub paused: bool,
    pub balls: Vec<Ball>,
}

impl Default for World {
    fn default() -> Self {
        let ball = |x, y, dx, dy| Ball { x, y, dx, dy };
        World {
            width: 640.0,
            height: 360.0,
            paused: false,
            balls: vec![ball(40.0, 40.0, 2.5, 1.7), ball(300.0, 120.0, -1.9, 2.2), ball(500.0, 260.0, 1.3, -2.6)],
        }
    }
}

/// The world a frame on (pure): each ball moved by its speed, turned back
/// at a wall with its speed kept.
pub fn step(w: &World) -> Vec<Ball> {
    w.balls
        .iter()
        .map(|b| {
            let (x, y) = (b.x + b.dx, b.y + b.dy);
            let dx = if x < 0.0 || x > w.width { -b.dx } else { b.dx };
            let dy = if y < 0.0 || y > w.height { -b.dy } else { b.dy };
            Ball { x, y, dx, dy }
        })
        .collect()
}

#[aio::methods]
impl World {
    /// A frame on, unless paused.
    pub fn tick(&mut self) {
        if !self.paused {
            self.balls = step(self);
        }
    }

    pub fn toggle(&mut self) {
        self.paused = !self.paused;
    }

    pub fn resize(&mut self, width: f64, height: f64) {
        (self.width, self.height) = (width, height);
    }
}

aio::client!(World, world);

/// A frame's drawing as one script on the canvas `el` (pure): cleared,
/// then a disc a ball, in the page's text colour.
pub fn draw(balls: &[Ball]) -> String {
    let discs: String =
        balls.iter().map(|b| format!("c.beginPath();c.arc({},{},12,0,6.2832);c.fill();", b.x, b.y)).collect();
    format!(
        "const c=el.getContext('2d');c.clearRect(0,0,el.width,el.height);c.fillStyle=getComputedStyle(el).color;{discs}"
    )
}

/// The window: the canvas, a frame drawn each time the balls move, and
/// Pause / Play. The loop moves the cell (`tick`), so every window of it
/// shows the same world.
#[component]
pub fn App() -> View {
    use_live();
    let (balls, paused) = (part(|w| &w.balls), part(|w| &w.paused));
    let (width, height) = (part(|w| &w.width), part(|w| &w.height));
    let canvas = signal(|| None::<std::rc::Rc<MountedData>>);
    aio::kit::use_frame(memo(move || !paused.get()), move |_, _| _ = act!(tick()));
    effect(move || {
        let js = balls.with(|b| draw(b));
        if let Some(el) = canvas.get() {
            el.run(&js);
        }
    });
    let theme = aio::kit::theme_css(aio::hue!());
    rsx! {
        style { {aio::use_css!(aio::ui::scheme(), &theme; "assets/style.css")} }
        main {
            h1 { "Canvas" }
            canvas { "data-t": "canvas", width: "{width}", height: "{height}", style: "width:100%",
                onmounted: move |e| *canvas.write() = Some(e.data()) }
            div { class: "row",
                button { r#type: "button", class: aio::ui::btn("primary"), "data-t": "toggle",
                    onclick: move |_| _ = act!(toggle()),
                    if paused.get() { "Play" } else { "Pause" }
                }
                span { "data-t": "bodies", {format!("{} bodies", balls.with(Vec::len))} }
            }
        }
    }
}

/// The app in a browser (`ax dev --web`, `ax build --web`): the same window.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn web() {
    aio::web::launch(App);
}

#[cfg(test)]
mod tests {
    use super::*;
    use aio::testing::TestUi;

    fn world(balls: Vec<Ball>) -> World {
        World { width: 100.0, height: 100.0, paused: false, balls }
    }

    #[test]
    fn a_ball_moves_by_its_speed() {
        let b = Ball { x: 10.0, y: 10.0, dx: 2.0, dy: 3.0 };
        assert_eq!(step(&world(vec![b])), [Ball { x: 12.0, y: 13.0, ..b }]);
    }

    #[test]
    fn a_ball_turns_back_at_each_wall_with_its_speed_kept() {
        let right = step(&world(vec![Ball { x: 99.0, y: 10.0, dx: 5.0, dy: 0.0 }]));
        assert_eq!(right[0].dx, -5.0, "turned, not stopped");
        let top = step(&world(vec![Ball { x: 10.0, y: 1.0, dx: 0.0, dy: -5.0 }]));
        assert_eq!(top[0].dy, 5.0);
    }

    #[test]
    fn a_paused_world_stays_and_the_frame_draws_each_ball() {
        let h = aio::Handle::<World>::memory();
        h.toggle().unwrap();
        h.tick().unwrap();
        assert_eq!(h.state().balls, World::default().balls, "paused");
        let js = draw(&h.state().balls);
        assert_eq!((js.matches("c.arc(").count(), js.contains("c.arc(40,40,12")), (3, true));
    }

    #[test]
    fn the_window_moves_the_world_until_paused() {
        let mut ui = TestUi::new(App);
        assert_eq!(ui.text("bodies"), "3 bodies");
        let x = || HANDLE.read_untracked().state().balls[0].x;
        let start = x();
        ui.wait(std::time::Duration::from_millis(200));
        assert_ne!(x(), start, "the frames move it");
        let inked = "const el = document.querySelector('[data-t=canvas]'); \
            const d = el.getContext('2d').getImageData(0, 0, el.width, el.height).data; \
            let n = 0; for (let i = 3; i < d.length; i += 4) if (d[i]) n++; return n;";
        assert!(ui.eval(&format!("(() => {{ {inked} }})()")).as_u64() > Some(0), "the balls are drawn");
        ui.click("toggle");
        assert_eq!(ui.text("toggle"), "Play");
        let held = x();
        ui.wait(std::time::Duration::from_millis(200));
        assert_eq!(x(), held, "paused");
    }
}

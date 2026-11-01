//! The app: its cell (the state, saved, and the only ways to change it)
//! and its window. `ax dev` hot-patches all of this library.
use aio::act;
use dioxus::prelude::*;

/// What the app keeps: each change shown at once and saved within 100 ms
/// (`coalesce`: aio's debounced writes), its history kept (`ax events`).
/// A field of a type of your own: put `#[aio::data]` on that type (it is
/// saved and sent; aio brings serde, so no dependency to add).
#[aio::cell(persist, coalesce, id = "counter")]
pub struct Counter {
    pub count: i64,
}

#[aio::methods]
impl Counter {
    /// Count `by` more (or less).
    pub fn add(&mut self, by: i64) {
        self.count += by;
    }

    pub fn reset(&mut self) {
        self.count = 0;
    }
}

aio::client!(Counter, counter);

/// The window: the count, and the buttons that change it. `part(|c| &c.count)`
/// re-renders a view only when that field changes (aio's signal per cell).
#[component]
pub fn App() -> Element {
    use_live();
    let n = *part(|c| &c.count);
    // The app's one hue (`[ui] hue`, else its id's): its icon's and splash's too.
    let theme = aio::kit::theme_css(aio::hue!());
    rsx! {
        // `false`: the light theme (`true`: dark). First the kit's theme (what
        // `aio::ui::btn` and the kit's components are drawn with), then yours.
        style { {aio::use_css!(false, &theme; "assets/style.css")} }
        main {
            h1 { "data-t": "count", "{n}" }
            button { class: aio::ui::btn("primary"), onclick: move |_| _ = act!(add(1)), "Add" }
            button { class: aio::ui::btn("ghost"), onclick: move |_| _ = act!(add(-1)), "Subtract" }
            button { class: aio::ui::btn("ghost"), disabled: n == 0, onclick: move |_| _ = act!(reset()), "Reset" }
        }
    }
}

/// The app in a browser (`ax dev --web`, `ax build --web`): the same
/// window, its cell kept in the browser's storage under the app's id.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn web() {
    let id = option_env!("AIORT_APP_ID").unwrap_or(env!("CARGO_PKG_NAME"));
    _ = SOURCE.set(aio::Source::Local(aio::Handle::saved(id)));
    aio::web::launch(App);
}

#[cfg(test)]
mod tests {
    use super::*;
    use aio::testing::TestUi;

    #[test]
    fn the_buttons_count_and_reset() {
        let mut ui = TestUi::new(App);
        assert_eq!(ui.text("count"), "0");
        assert!(ui.disabled("ResetButton"), "nothing to reset");
        ui.click("AddButton");
        ui.click("AddButton");
        ui.click("SubtractButton");
        assert_eq!(ui.text("count"), "1");
        ui.click("ResetButton");
        assert_eq!(ui.text("count"), "0");
    }
}

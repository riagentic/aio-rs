//! The `thing` page (`ax add page thing`): a crate of its own, so `ax dev`
//! recompiles only this crate for an edit here, however big the app grows.
//! Show it from the app's view: `thing::Thing {}`.
use aio::ui::prelude::*;

#[component]
pub fn Thing() -> View {
    rsx! {
        section { class: "page thing",
            h2 { "Thing" }
        }
    }
}

//! A client's window: the app, or its connect page while the client has no
//! server it can reach (`aio::connect`, `aio::kit::ConnectPage`): the same
//! window, desktop and Android.
use aio::ui::prelude::*;
use std::sync::OnceLock;

/// The app's root, its data dir and its id, for a window's client.
type Gate = (fn() -> View, std::path::PathBuf, String);
static GATE: OnceLock<Gate> = OnceLock::new();

/// The window's root: `app`, gated on the connect page when this process
/// has a window's client (`Source::for_app` in client mode, with the kit).
pub(crate) fn root(app: fn() -> View, dirs: &aio::paths::AppDirs) -> fn() -> View {
    if !aio::connect::active() {
        return app;
    }
    _ = GATE.set((app, dirs.data.clone(), dirs.id.clone()));
    gated
}

fn gated() -> View {
    let Some((_, data, id)) = GATE.get().cloned() else { return rsx! {} };
    let why = aio::kit::use_connect_page();
    let hue = crate::instance::LOOK.get().map_or_else(|| aio::colors::hue(&id), |l| l.0);
    let css = aio::ui::page_css(&[&aio::kit::theme_css(hue)], aio::ui::scheme());
    rsx! {
        if why.with(Option::is_some) {
            style { {css.clone()} }
            aio::kit::ConnectPage { data: data.clone(), app: id.clone() }
        } else {
            App {}
        }
    }
}

/// The app's root as a component: its hooks get a body of their own.
#[component]
fn App() -> View {
    GATE.get().map_or_else(|| rsx! {}, |g| (g.0)())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aio::testing::TestUi;

    fn app() -> View {
        let n = once(|| 7);
        rsx! { p { "data-t": "shown", "app {n}" } }
    }

    /// The app shows in the gate's place, its hooks in a body: no page, the app.
    #[test]
    fn a_gated_window_shows_the_app_with_its_hooks() {
        let d = tempfile::tempdir().unwrap();
        _ = GATE.set((app, d.path().to_path_buf(), "board".into()));
        let ui = TestUi::new(gated);
        assert_eq!(ui.text("shown"), "app 7");
    }
}

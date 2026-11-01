//! The guide's pages (`docs/`) whose Rust calls this crate (dialogs, the
//! tray, launches), so every example in them compiles, and runs unless
//! marked `no_run`, as a doctest of aio-rs-shell. `scripts/docs-check`
//! fails when a page with Rust in it is in neither guide.

macro_rules! pages {
    ($($(#[$m:meta])* $name:ident = $path:literal),* $(,)?) => {
        $( $(#[$m])* #[doc = include_str!(concat!("../../../docs/", $path))] pub struct $name; )*
    };
}

pages! {
    ExDesktopApp = "examples/desktop-app.md",
}

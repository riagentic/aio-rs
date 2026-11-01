//! Written by `ax surface --headless` for one run, then removed: the
//! app's views, on its running cells.
#[test]
fn aio_surface() {
    // Set by `ax surface --headless`; without them there is nothing to show.
    let (Ok(app), Ok(data), Ok(user)) = (
        std::env::var("AIO_SURFACE_APP"),
        std::env::var("AIO_SURFACE_DATA"),
        std::env::var("AIO_SURFACE_USER"),
    ) else {
        return;
    };
    println!("\n--- ax surface ---\n{}\n--- ax surface end ---", surface(&app, &data, &user));
}

/// Cell `T`'s live copy, over the running app's local door.
fn live<T: aio::Cell>(
    _: &dioxus::prelude::GlobalSignal<aio::Source<T>>,
    app: &str,
    data: &str,
    user: &str,
) -> aio::Source<T> {
    let door = aio::sync::door(app, std::path::Path::new(data), T::ID).expect("the app's door");
    aio::Source::connect(&door, "", user).unwrap_or_else(|e| panic!("{}: {e}", T::ID))
}

/// `headless::tests::Books` on the live cells, as a person meets it.
pub fn surface(app: &str, data: &str, user: &str) -> String {
    let c0 = live(&crate::headless::tests::HANDLE, app, data, user);
    let ui = aio::testing::Mount::default()
        .before(move || {
            *crate::headless::tests::HANDLE.write() = c0;
        })
        .app(crate::headless::tests::Books);
    ui.surface()
}

//! The UI kit (aio's `aio/ui`): the controls every app otherwise rewrites, as
//! air components over real elements (feature `kit`), styled by one stylesheet ([`CSS`])
//! and themed by `--aio-*` tokens ([`theme_css`]). Keyboard first: each
//! control does its WAI-ARIA keyboard pattern. Every component takes `class`
//! (appended to its own) and any other attribute (`"data-t"`, `id`, `aria-*`),
//! which wins over the kit's own. The parts a person uses inside a control
//! carry handles of their own: a tab's or menu item's id, `menuTrigger`,
//! `menuScrim`, `modalBackdrop`, `confirmOk`, `confirmCancel`, `toastDismiss`,
//! `alertDismiss`, `pagePrev`, `pageNext`, `page{n}`, `zoomIn`, `zoomOut`,
//! `zoomFit`, `zoomStage`, and the sign-in form's `signin…` parts.
//!
//! On air a component's body runs once: a prop that may change is a
//! [`Prop`](crate::ui::prelude::Prop) (`value: name.get()` follows `name`),
//! and the parts of a control that show it are its holes.

mod back;
mod browser;
mod change;
#[cfg(not(target_arch = "wasm32"))]
mod connect;
mod controls;
mod defer;
mod form;
mod forms;
mod held;
mod island;
mod keys;
mod layout;
pub mod markdown;
mod modal;
mod motion;
mod portal;
#[cfg(any(feature = "server", target_arch = "wasm32"))]
mod signin;
pub mod theme;
mod transition;
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub mod travel;
mod vlist;
mod window;
mod zoom;

pub use back::use_back_button;
pub use browser::{Browser, Go, GuestEvent, HostKey, go, run_js};
pub use change::{use_on_change, use_on_change_now};
#[cfg(not(target_arch = "wasm32"))]
pub use connect::{ConnectPage, use_connect_page};
pub use controls::*;
pub use defer::{Defer, Load, Trigger, load};
pub use form::*;
pub use forms::{Bind, FieldArray, FieldOf, Form, Problems, move_item, use_async_rule, use_field_array, use_form};
pub use held::use_held;
pub use island::{Island, WebIsland};
pub use keys::{
    Chord, ChordSpec, GlobalKeys, Hit, MOVE_MS, field_key, keycap_match, normalize_key, use_global_key, use_key_claim,
};
pub use layout::*;
pub use markdown::Markdown;
pub use modal::*;
pub use motion::{FRAME, Spring, SpringValue, spring_step, use_frame, use_interval, use_spring};
pub use portal::{Portal, PortalHost};
#[cfg(any(feature = "server", target_arch = "wasm32"))]
pub use signin::SignIn;
pub use transition::{Motion, Transition, TransitionGroup, merge_rows, moved};
#[cfg(not(target_arch = "wasm32"))]
pub use travel::{DevTimeTravel, TimeTravelPanel};
pub use vlist::{VirtualList, window};
pub use window::{WindowEvent, use_window_event, use_window_listener};
pub use zoom::{ZoomPan, ZoomView, fit_view, zoom_at};

/// The kit's stylesheet: the theme's visual rules (`@layer aio`, so any app
/// rule wins) and every component's classes. Hue-free: the colours are tokens.
pub const CSS: &str = include_str!("../../assets/kit.css");

/// The whole default theme for an app: its tokens (hue from its id, see
/// [`theme::hue`]) and the kit. Pass it to [`crate::ui::page_css`] first.
/// In the browser the kit's part comes without comments or needless
/// spaces (a quarter smaller, the same rules): the page fetches it.
pub fn theme_css(hue: u16) -> String {
    #[cfg(target_arch = "wasm32")]
    let css = crate::shrunk!(css, CSS);
    #[cfg(not(target_arch = "wasm32"))]
    let css = CSS;
    theme::tokens_css(hue, false) + "\n" + css
}

pub(crate) use crate::ui::clock::{Instant, sleep};

/// A checkbox's state and handler: a bound one's from its form's field
/// (an edit writes and touches it), else the props as given.
fn bound_check(
    bind: Option<Bind>,
    checked: crate::ui::prelude::Prop<bool>,
    onchange: Option<crate::ui::prelude::EventHandler<bool>>,
) -> (crate::ui::prelude::Prop<bool>, Option<crate::ui::prelude::EventHandler<bool>>) {
    use crate::ui::prelude::*;
    let Some(b) = bind else { return (checked, onchange) };
    let shown = b.clone();
    let on = EventHandler::new(move |v: bool| {
        b.input(v.to_string());
        b.blur();
        if let Some(f) = onchange {
            f.call(v)
        }
    });
    (Prop::from(memo(move || shown.value() == "true")), Some(on))
}

/// The kit's class, then the caller's.
fn join(base: &str, extra: &str) -> String {
    #[cfg(feature = "dev")]
    note_unstyled(base);
    if extra.is_empty() { base.into() } else { format!("{base} {extra}") }
}

/// Why kit class `cls` shows as bare HTML on a page styled by `page`, if it
/// does: the kit's sheet styles `cls` and nothing on the page names it (the
/// kit's sheet, an adopted copy and the app's own rule all do; aio
/// `_kitStyled`).
pub fn unstyled(page: &str, cls: &str) -> Option<String> {
    let sel = format!(".{cls}");
    (CSS.contains(&sel) && !page.contains(&sel)).then(|| {
        format!(
            "a kit control (class \"{cls}\") rendered, but nothing on the page styles the kit: it shows as bare \
             HTML. Pass aio::kit::theme_css(..) (or the app's adopted aio-theme.css) to aio::ui::page_css. \
             Styling the kit in the app's own CSS? Any rule naming {sel} counts."
        )
    })
}

/// Say [`unstyled`] once per run, in debug builds, at the first control the
/// kit's sheet styles (aio's `_noteKitRender`).
#[cfg(feature = "dev")]
fn note_unstyled(base: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static CHECKED: AtomicBool = AtomicBool::new(false);
    let cls = base.split(' ').next().unwrap_or(base);
    if CHECKED.load(Ordering::Relaxed) || !CSS.contains(&format!(".{cls}")) {
        return;
    }
    CHECKED.store(true, Ordering::Relaxed);
    if let Some(why) = unstyled(&crate::ui::page_sheet(), cls) {
        crate::warn!(cat: "ui", "{why}");
    }
}

/// A stable id per component instance (for `aria-controls`, `for` and
/// `name` pairs): made once.
pub fn use_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    crate::ui::once(|| format!("aio-{prefix}-{}", NEXT.fetch_add(1, Ordering::Relaxed)))
}

/// A part of a context (aio's `useContextSelector`): the provider gives a
/// `Signal<T>` (`provide(|| Signal::new(t))`), and what reads the memo
/// follows only `f`'s answer, not every change to `T`.
/// With no provider above, `f` reads `T::default()` (aio's context default).
pub fn use_context_select<T: Default + 'static, R: PartialEq + 'static>(
    f: impl Fn(&T) -> R + 'static,
) -> crate::ui::Memo<R> {
    use crate::ui::prelude::*;
    let ctx = once(try_context::<Signal<T>>);
    memo(move || match ctx {
        Some(s) => f(&s.read()),
        None => f(&T::default()),
    })
}

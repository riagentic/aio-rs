//! Deferred content (aio's `Defer`): a placeholder until the trigger fires,
//! then the content, for good (one-shot per mount; a new `key` defers
//! again). An app's code is all in its binary, so there is no module to
//! fetch: what waits is the render, and data the content loads itself
//! (`use_resource`, [`use_held`](super::use_held)) with its own loading
//! and error states.
use super::join;
use crate::ui::prelude::*;
use std::time::Duration;

/// When [`Defer`] shows its content.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Trigger {
    Now,
    After(Duration),
    /// Once the first frame is drawn and the view has nothing else to do.
    Idle,
    /// When the placeholder comes into the window.
    Viewport,
    /// When the pointer comes over the placeholder.
    Hover,
    /// At a click or a key on the placeholder.
    Interaction,
}

/// `placeholder` (in a box of its own: `class`, attributes) until
/// `trigger` fires, then `children`.
#[component]
pub fn Defer(
    trigger: Trigger,
    placeholder: Option<View>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: View,
) -> View {
    let on = signal(|| trigger == Trigger::Now);
    once(|| {
        let wait = match trigger {
            Trigger::After(d) => Some(d),
            Trigger::Idle => Some(super::FRAME),
            _ => None,
        };
        if let Some(d) = wait {
            spawn(async move {
                super::sleep(d).await;
                tokio::task::yield_now().await;
                on.set(true);
            });
        }
    });
    if on.get() {
        return children;
    }
    let fire = move |when: Trigger| {
        if when == trigger {
            on.set(true)
        }
    };
    rsx! {
        div {
            class: join("aio-defer", &class),
            onvisible: move |e| {
                if e.is_intersecting().unwrap_or(false) {
                    fire(Trigger::Viewport)
                }
            },
            onmousemove: move |_| fire(Trigger::Hover),
            onclick: move |_| fire(Trigger::Interaction),
            onkeydown: move |_| fire(Trigger::Interaction),
            ..attributes,
            {placeholder}
        }
    }
}

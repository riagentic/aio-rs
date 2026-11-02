//! Dialogs and notices (aio's `Modal`, `Confirm`, `ConfirmButton`, `toast`,
//! `ToastHost`). Escape closes the most recently opened dialog and only that
//! one (a non-dismissable one on top blocks it); focus moves into a dialog
//! when it opens and Tab stays inside it. In the window, the shell's page
//! script does what needs the DOM (`aio_shell` `DIALOG`): the first control
//! takes focus, Shift+Tab wraps to the last, and the opener gets it back.
use super::controls::Tone;
use super::form::{Button, Size, Variant, btn_class};
use super::{join, use_id};
use crate::ui::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

thread_local! {
    /// The open dialogs, in the order they opened, each with how it closes
    /// (none: not dismissable).
    static OPEN: RefCell<Vec<(String, Option<EventHandler<()>>)>> = const { RefCell::new(Vec::new()) };
}

fn top_close() -> Option<Option<EventHandler<()>>> {
    OPEN.with(|o| o.borrow().last().map(|(_, c)| *c))
}

/// A dialog over a backdrop: Escape or a backdrop click call `onclose` (when
/// `dismissable`). It renders nothing while closed; its children are built
/// each time it opens.
#[component]
pub fn Modal(
    open: Prop<bool>,
    onclose: Option<EventHandler<()>>,
    /// Text or any view; it names the dialog.
    title: Option<View>,
    footer: Option<View>,
    #[props(default = true)] dismissable: Prop<bool>,
    #[props(default, into)] class: Prop<String>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: View,
) -> View {
    let id = use_id("modal");
    let close = memo(move || onclose.filter(|_| dismissable.get()));
    // Open order, not mount order: a dialog opened later is on top.
    let shown = memo(move || open.get());
    let was = once(|| Rc::new(Cell::new(false)));
    {
        let id = id.clone();
        effect(move || {
            let (open, close) = (shown.get(), close.get());
            OPEN.with(|o| {
                let mut o = o.borrow_mut();
                match (open, was.replace(open)) {
                    (true, false) => o.push((id.clone(), close)),
                    (false, true) => o.retain(|(k, _)| *k != id),
                    (true, true) => o.iter_mut().filter(|(k, _)| *k == id).for_each(|e| e.1 = close),
                    _ => {}
                }
            });
        });
    }
    let gone = id.clone();
    on_cleanup(move || OPEN.with(|o| o.borrow_mut().retain(|(k, _)| *k != gone)));
    let dialog = signal(|| None::<Rc<MountedData>>);
    let to_box = move |_| {
        if let Some(el) = dialog.read_untracked().clone() {
            spawn(async move { _ = el.set_focus(true).await });
        }
    };
    rsx! {
        if shown.get() {
            div {
                class: join("aio-modal-backdrop", &class.get()),
                role: "presentation",
                "data-t": "modalBackdrop",
                onclick: move |_| if let Some(f) = close.get_untracked() { f.call(()) },
                onkeydown: move |e| {
                    if e.key() == Key::Escape && !e.is_composing() && let Some(top) = top_close() {
                        e.stop_propagation();
                        if let Some(f) = top {
                            f.call(())
                        }
                    }
                },
                ..attributes.clone(),
                // Focus guards: Tab past either end comes back to the dialog.
                span { tabindex: "0", onfocus: to_box }
                div {
                    class: "aio-modal",
                    role: "dialog",
                    "aria-modal": "true",
                    "aria-labelledby": title.is_some().then(|| format!("{id}-title")),
                    tabindex: "-1",
                    onmounted: move |e| async move {
                        *dialog.write() = Some(e.data());
                        _ = e.set_focus(true).await;
                    },
                    // A click in the dialog is not a click on the backdrop.
                    onclick: |e| e.stop_propagation(),
                    if title.is_some() {
                        div { class: "aio-modal__title", id: "{id}-title", {title.clone()} }
                    }
                    div { class: "aio-modal__body", {children.clone()} }
                    if footer.is_some() {
                        div { class: "aio-modal__footer", {footer.clone()} }
                    }
                }
                span { tabindex: "0", onfocus: to_box }
            }
        }
    }
}

/// "Are you sure?": a dialog with a cancel and a confirm button. Escape and
/// the backdrop cancel.
#[component]
pub fn Confirm(
    open: Prop<bool>,
    onconfirm: EventHandler<()>,
    oncancel: EventHandler<()>,
    /// Text or any view, as are the message and the labels.
    title: Option<View>,
    message: Option<View>,
    #[props(default = ::air::text_view(|| "Confirm".into()))] confirm_label: View,
    #[props(default = ::air::text_view(|| "Cancel".into()))] cancel_label: View,
    /// The confirm button is destructive.
    #[props(default)]
    danger: Prop<bool>,
    #[props(default, into)] class: Prop<String>,
) -> View {
    rsx! {
        Modal { open, onclose: oncancel, title, class,
            footer: rsx! {
                Button { variant: Variant::Ghost, "data-t": "confirmCancel", onclick: move |_| oncancel.call(()), {cancel_label} }
                Button {
                    variant: if danger.get() { Variant::Danger } else { Variant::Primary },
                    "data-t": "confirmOk",
                    onclick: move |_| onconfirm.call(()),
                    {confirm_label}
                }
            },
            if message.is_some() {
                div { class: "aio-confirm__msg", {message.clone()} }
            }
        }
    }
}

/// A button that asks first: `onconfirm` runs only once the dialog is
/// confirmed. It takes a [`Button`]'s props (`variant`, `size`, `disabled`,
/// `class`, attributes); a danger button makes the confirm button danger
/// too. Focus returns to the button when the dialog closes.
#[component]
pub fn ConfirmButton(
    /// What it asks: text or any view, as are the title and the labels.
    confirm: View,
    onconfirm: EventHandler<()>,
    confirm_title: Option<View>,
    #[props(default = ::air::text_view(|| "Confirm".into()))] confirm_label: View,
    #[props(default = ::air::text_view(|| "Cancel".into()))] cancel_label: View,
    #[props(default)] variant: Prop<Variant>,
    #[props(default)] size: Prop<Size>,
    #[props(default)] disabled: Prop<bool>,
    #[props(default, into)] class: Prop<String>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
    children: View,
) -> View {
    let open = signal(|| false);
    let me = signal(|| None::<Rc<MountedData>>);
    let done = move || {
        open.set(false);
        if let Some(el) = me.read_untracked().clone() {
            spawn(async move { _ = el.set_focus(true).await });
        }
    };
    rsx! {
        button {
            r#type: "button",
            class: btn_class(variant.get(), size.get(), &class.get()),
            disabled,
            onmounted: move |e| *me.write() = Some(e.data()),
            onclick: move |_| open.set(true),
            ..attributes,
            {children}
        }
        Confirm {
            open: open.get(),
            title: confirm_title,
            message: confirm,
            confirm_label,
            cancel_label,
            danger: variant.get() == Variant::Danger,
            oncancel: move |_| done(),
            onconfirm: move |_| {
                done();
                onconfirm.call(());
            },
        }
    }
}

/// A shown notice.
#[derive(Clone, Debug, PartialEq)]
struct Notice {
    id: u64,
    message: View,
    /// Its text, when it is text.
    text: Option<String>,
    tone: Tone,
}

static TOASTS: Global<Vec<Notice>> = Global::named(Vec::new, "aio::kit::TOASTS");

thread_local! {
    static NEXT: Cell<u64> = const { Cell::new(0) };
    /// Each notice's auto-dismiss timer, cancelled however it goes.
    static TIMERS: RefCell<Vec<(u64, Task)>> = const { RefCell::new(Vec::new()) };
}

/// A toast, to dismiss it early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Toast(u64);

impl Toast {
    /// The one way a toast goes (its ×, its timer, this): the timer goes too.
    pub fn dismiss(self) {
        if let Some(task) = self.forget() {
            task.cancel();
        }
    }

    /// Take it off the screen; its timer, if it has one still waiting.
    fn forget(self) -> Option<Task> {
        TOASTS.write().retain(|n| n.id != self.0);
        TIMERS.with(|t| {
            let mut t = t.borrow_mut();
            let at = t.iter().position(|(id, _)| *id == self.0)?;
            Some(t.remove(at).1)
        })
    }
}

/// The notices showing now, oldest first (for tests and agents).
pub fn toasts() -> Vec<String> {
    TOASTS.read().iter().map(|n| n.text.clone().unwrap_or_default()).collect()
}

/// Show a notice in the [`ToastHost`] for 4 seconds. Call it on the UI thread
/// (a handler, a view's task).
pub fn toast(message: impl Into<String>, tone: Tone) -> Toast {
    toast_for(message, tone, Some(Duration::from_secs(4)))
}

/// Show a notice for `stay` (`None`: until dismissed).
pub fn toast_for(message: impl Into<String>, tone: Tone, stay: Option<Duration>) -> Toast {
    let text: String = message.into();
    let view = rsx! { "{text}" };
    show(view, Some(text), tone, stay)
}

/// Show any view as a notice for `stay` (`None`: until dismissed): a link,
/// a bold word, an icon.
pub fn toast_view(message: View, tone: Tone, stay: Option<Duration>) -> Toast {
    show(message, None, tone, stay)
}

fn show(message: View, text: Option<String>, tone: Tone, stay: Option<Duration>) -> Toast {
    let id = NEXT.with(|n| n.replace(n.get() + 1));
    TOASTS.write().push(Notice { id, message, text, tone });
    if let Some(d) = stay {
        let task = spawn_forever(async move {
            super::sleep(d).await;
            _ = Toast(id).forget();
        });
        TIMERS.with(|t| t.borrow_mut().push((id, task)));
    }
    Toast(id)
}

/// Where toasts show: once, at the app root (a polite live region; an
/// error is an alert).
#[component]
pub fn ToastHost(
    #[props(default, into)] class: Prop<String>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    rsx! {
        div { class: join("aio-toasts", &class.get()), role: "status", "aria-live": "polite", ..attributes,
            for n in TOASTS.get() {
                div { key: "{n.id}", class: "aio-toast aio-toast--{n.tone.key()}", role: (n.tone == Tone::Error).then_some("alert"),
                    span { class: "aio-toast__msg", {n.message.clone()} }
                    button { r#type: "button", class: "aio-toast__x", "aria-label": "Dismiss", "data-t": "toastDismiss", onclick: move |_| Toast(n.id).dismiss(), "×" }
                }
            }
        }
    }
}

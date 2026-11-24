//! `QrScanner`: the camera's picture, and the text of each QR code in it.
//! The page asks for the camera (`getUserMedia`, the back one first) and
//! reads codes with its browser's own reader where there is one
//! (`BarcodeDetector`: Chromium on macOS, Android, ChromeOS); elsewhere
//! (Electron on Linux and Windows, WebKit, Firefox) it hands grey frames,
//! at most 480 px a side and one at a time, to [`decode_qr`], aio's own
//! reader, in the app. An app that never shows one builds none of it.
mod decode;

use super::join;
use crate::ui::prelude::*;
pub use decode::decode_qr;

/// Where a [`QrScanner`]'s camera is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QrState {
    /// Asking for the camera (the system's prompt may be up).
    Starting,
    /// Showing the camera, reading codes.
    Scanning,
    /// Refused: by the person, or by the app (a window without `"media"`
    /// in its permissions; on Android, no `[build.android] camera = true`).
    Denied,
    /// No camera here, or none a page can reach (not https or localhost).
    NoCamera,
    /// The camera failed (taken by another app) or stopped (unplugged).
    Failed(String),
}

/// The script, on the box (`el`), its comment lines left out.
const SCRIPT: &str = crate::shrunk!(js, include_str!("qr.js"));

/// The camera's picture (sized by `class` or `style`; it fills its box),
/// and `onscan` with the text of each QR code it sees: a text once, again
/// only after another. Without a camera it says why instead (`onstate`
/// hears it too: offer to paste the text). The camera is let go when it
/// leaves the view.
///
/// The window must allow the camera: `Window { permissions: &["media"] }`;
/// on Android also `[build.android] camera = true`; in a browser the page
/// must be https (or localhost). The system asks the person once.
#[component]
pub fn QrScanner(
    onscan: EventHandler<String>,
    #[props(default)] onstate: EventHandler<QrState>,
    #[props(default, into)] class: String,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    let state = signal(|| QrState::Starting);
    let last = signal(|| None::<String>);
    let set = move |s: QrState| {
        state.set(s.clone());
        onstate.call(s);
    };
    let found = move |text: String| {
        if last.with_untracked(|l| l.as_deref() != Some(&text)) {
            last.set(Some(text.clone()));
            onscan.call(text);
        }
    };
    let note = memo(move || match state.get() {
        QrState::Starting | QrState::Scanning => String::new(),
        QrState::Denied => "The camera is not allowed. Allow it in the system's settings, then try again.".into(),
        QrState::NoCamera => "No camera found.".into(),
        QrState::Failed(why) => format!("The camera failed: {why}"),
    });
    rsx! {
        div {
            class: join("aio-qr", &class),
            onmounted: move |e| async move {
                let page = e.data().eval(SCRIPT);
                // A letter, then its text (the script says which).
                while let Ok(m) = page.recv_json().await {
                    let said = match serde_json::from_str(&m) {
                        Ok(serde_json::Value::String(s)) => s,
                        _ => String::new(),
                    };
                    let (kind, text) = said.split_at(said.len().min(1));
                    match kind {
                        "o" => set(QrState::Scanning),
                        "t" => found(text.into()),
                        "f" => {
                            use base64::Engine;
                            let b = base64::engine::general_purpose::STANDARD.decode(text).unwrap_or_default();
                            if let [w0, w1, h0, h1, luma @ ..] = &b[..] {
                                let (w, h) = (u16::from_be_bytes([*w0, *w1]), u16::from_be_bytes([*h0, *h1]));
                                if let Some(t) = decode_qr(luma, w.into(), h.into()) {
                                    found(t);
                                }
                            }
                            _ = page.send_json("0");
                        }
                        "d" => {
                            crate::warn!(cat: "ui", "[QrScanner] camera denied: {text}");
                            set(QrState::Denied);
                        }
                        "n" => {
                            crate::warn!(cat: "ui", "[QrScanner] no camera: {text}");
                            set(QrState::NoCamera);
                        }
                        "x" => set(QrState::Failed(text.into())),
                        _ => {}
                    }
                }
            },
            ..attributes,
            if !note.get().is_empty() {
                p { class: "aio-qr__note", role: "status", "{note}" }
            }
        }
    }
}

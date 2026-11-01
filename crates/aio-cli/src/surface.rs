//! `ax surface <app> [path]` and `ax trigger <app> <path> <action> [text]`
//! (aio's `am surface`, `am trigger`): the live window read and driven as a
//! person meets it, over the window's DevTools protocol (as `aio
//! shot`). Elements are named as `aio::testing::TestUi` names them (a
//! `data-t` handle, else an auto name: `SaveButton`, `EmailInput`,
//! `SaveButton2`; `row-2/DeleteButton` inside another), so a test's paths
//! work on the running app. The surface is read by `surface.js` in the
//! page; a trigger acts with real input (mouse and keys through Chromium),
//! so the app's handlers run as for a person.
use crate::cdp::Session;
use aio::serde_json::{Value, json};

/// The page side: the surface, the names, and the DOM steps of a trigger.
const SCRIPT: &str = aio::ui::SURFACE_JS;

/// What `ax trigger` does (aio's `TRIGGER_ACTIONS`), and whether each takes a text.
pub const ACTIONS: &[(&str, bool)] = &[
    ("click", false),
    ("dblclick", false),
    ("hover", false),
    ("focus", false),
    ("blur", false),
    ("type", true),
    ("setValue", true),
    ("clear", false),
    ("press", true),
    ("keyDown", true),
    ("keyUp", true),
    ("select", true),
    ("check", false),
    ("uncheck", false),
    ("scroll", true),
    ("dragTo", true),
];

/// The path a key action takes to aim at the window, not an element (an
/// app's `aio::kit::use_global_key`; aio's `ui.window`).
pub const WINDOW: &str = "window";

/// One step of `surface.js` in the page: its answer, or its refusal.
async fn page(s: &mut Session, op: &str, arg: Value) -> Result<Value, String> {
    let r = s.eval(&format!("({SCRIPT})({}, {arg})", json!(op))).await?;
    match r.get("error").and_then(Value::as_str) {
        Some(e) => Err(e.into()),
        None => Ok(r["ok"].clone()),
    }
}

/// The surface under `path` (all of the window when `None`), one line each;
/// `rects`: where each is, `@x,y wxh` in CSS px.
pub async fn read(s: &mut Session, path: Option<&str>, rects: bool) -> Result<String, String> {
    Ok(page(s, "surface", json!({ "path": path, "rects": rects })).await?.as_str().unwrap_or_default().into())
}

/// The auto names in the window, in order.
pub async fn names(s: &mut Session) -> Result<Vec<String>, String> {
    Ok(page(s, "names", json!({}))
        .await?
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| n.as_str().map(String::from))
        .collect())
}

/// The box of the element at `path` in CSS px, `{x, y, w, h}` (for a video's markers).
pub async fn rect(s: &mut Session, path: &str) -> Result<Value, String> {
    let b = page(s, "box", json!({ "path": path })).await?;
    let (w, h) = (b["w"].as_f64().unwrap_or(0.0), b["h"].as_f64().unwrap_or(0.0));
    let at = |k: &str, d: f64| b[k].as_f64().unwrap_or(0.0) - d / 2.0;
    Ok(json!({ "x": at("x", w), "y": at("y", h), "w": w, "h": h }))
}

/// A key and its modifiers from a chord as `aio::kit::Chord` spells it
/// (`Enter`, `ctrl+shift+k`, `mod+s`: Ctrl, or ⌘ on macOS) (pure): CDP's
/// `key`, `code`, key code, the text it types and the modifier bits (Alt 1,
/// Ctrl 2, Meta 4, Shift 8).
pub fn key(spec: &str) -> Result<Value, String> {
    let c = aio::kit::Chord::parse(spec)?;
    let mac = cfg!(target_os = "macos");
    let (ctrl, meta) = (c.ctrl || (c.cmd && !mac), c.meta || (c.cmd && mac));
    let bits = u32::from(c.alt) | u32::from(ctrl) << 1 | u32::from(meta) << 2 | u32::from(c.shift) << 3;
    aio::testing::cdp_key(&c.key, bits)
        .ok_or_else(|| format!("{spec}: no key is named {} (Enter, Escape, ArrowLeft, a, …)", c.key))
}

async fn settle(s: &mut Session) -> Result<(), String> {
    s.eval(crate::cdp::PAINTED).await.map(|_| ())
}

async fn mouse(s: &mut Session, kind: &str, b: &Value, held: bool, clicks: u32) -> Result<(), String> {
    let p = json!({ "type": kind, "x": b["x"], "y": b["y"], "button": "left", "buttons": u8::from(held), "clickCount": clicks });
    s.call("Input.dispatchMouseEvent", p).await.map(|_| ())
}

async fn click(s: &mut Session, b: &Value, clicks: u32) -> Result<(), String> {
    mouse(s, "mouseMoved", b, false, 0).await?;
    for n in 1..=clicks {
        mouse(s, "mousePressed", b, true, n).await?;
        mouse(s, "mouseReleased", b, false, n).await?;
    }
    Ok(())
}

async fn keys(s: &mut Session, k: &Value, down: bool, up: bool) -> Result<(), String> {
    if down {
        let ty = if k["text"].as_str().is_some_and(|t| !t.is_empty()) { "keyDown" } else { "rawKeyDown" };
        let mut d = k.clone();
        d["type"] = ty.into();
        if ty == "rawKeyDown" {
            d.as_object_mut().map(|o| o.remove("text"));
        }
        s.call("Input.dispatchKeyEvent", d).await?;
    }
    if up {
        let mut u = k.clone();
        u["type"] = "keyUp".into();
        u.as_object_mut().map(|o| o.remove("text"));
        s.call("Input.dispatchKeyEvent", u).await?;
    }
    Ok(())
}

/// Do `action` (one of [`ACTIONS`]) to the element at `path`, with `text`,
/// as a person would; once the window painted what it did, what was done.
pub async fn trigger(s: &mut Session, path: &str, action: &str, text: Option<&str>) -> Result<String, String> {
    let Some((_, takes)) = ACTIONS.iter().find(|(a, _)| *a == action) else {
        let all: Vec<&str> = ACTIONS.iter().map(|(a, _)| *a).collect();
        return Err(format!("no action {action}: {}", all.join(", ")));
    };
    let text = match (takes, text) {
        (true, Some(t)) => t,
        (true, None) => return Err(format!("{action} needs a text: ax trigger <app> {path} {action} <text>")),
        (false, Some(t)) => return Err(format!("{action} takes no text ({t})")),
        (false, None) => "",
    };
    if path == WINDOW && !matches!(action, "press" | "keyDown" | "keyUp") {
        return Err(format!("{action} needs an element: the window takes press, keyDown and keyUp"));
    }
    let at = |verb: &str| json!({ "path": path, "verb": verb });
    match action {
        "click" | "dblclick" => {
            let b = page(s, "box", at(action)).await?;
            click(s, &b, if action == "click" { 1 } else { 2 }).await?;
        }
        "hover" => {
            let b = page(s, "box", at("hover")).await?;
            mouse(s, "mouseMoved", &b, false, 0).await?;
        }
        "focus" | "blur" => _ = page(s, action, at(action)).await?,
        "type" | "setValue" | "clear" => {
            page(s, "edit", json!({ "path": path, "verb": action, "all": action != "type" })).await?;
            match text {
                "" => keys(s, &key("Backspace")?, true, true).await?,
                t => _ = s.call("Input.insertText", json!({ "text": t })).await?,
            }
        }
        "press" | "keyDown" | "keyUp" => {
            let k = key(text)?;
            page(s, "focus", at(action)).await?;
            keys(s, &k, action != "keyUp", action != "keyDown").await?;
        }
        "select" => _ = page(s, "select", json!({ "path": path, "verb": "select", "value": text })).await?,
        "check" | "uncheck" => {
            let on = page(s, "checked", at(action)).await?;
            if on != (action == "check") {
                let b = page(s, "box", at(action)).await?;
                click(s, &b, 1).await?;
            }
        }
        "scroll" => {
            let (top, left) = text.split_once(',').unwrap_or((text, "0"));
            let n = |v: &str| v.trim().parse::<f64>().map_err(|_| format!("scroll {text}: top[,left] in pixels"));
            page(s, "scroll", json!({ "path": path, "top": n(top)?, "left": n(left)? })).await?;
        }
        "dragTo" => {
            let (from, to) = (page(s, "box", at("drag")).await?, page(s, "box", json!({ "path": text })).await?);
            mouse(s, "mouseMoved", &from, false, 0).await?;
            mouse(s, "mousePressed", &from, true, 1).await?;
            for i in 1..=8 {
                let f = f64::from(i) / 8.0;
                let lerp = |k: &str| from[k].as_f64().unwrap_or(0.0) * (1.0 - f) + to[k].as_f64().unwrap_or(0.0) * f;
                mouse(s, "mouseMoved", &json!({ "x": lerp("x"), "y": lerp("y") }), true, 0).await?;
            }
            mouse(s, "mouseReleased", &to, false, 1).await?;
        }
        _ => unreachable!("every action is matched"),
    }
    settle(s).await?;
    Ok(match text {
        "" => format!("{action} {path}"),
        t => format!("{action} {path} {t:?}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_named_as_the_dom_names_it_with_its_modifiers() {
        let k = key("Enter").unwrap();
        assert_eq!(
            (k["code"].as_str(), k["windowsVirtualKeyCode"].as_u64(), k["text"].as_str()),
            (Some("Enter"), Some(13), Some("\r"))
        );
        let k = key("Ctrl+Shift+k").unwrap();
        assert_eq!(
            (k["key"].as_str(), k["code"].as_str(), k["modifiers"].as_u64(), k["text"].as_str()),
            (Some("k"), Some("KeyK"), Some(10), Some(""))
        );
        assert_eq!(key("Shift+A").unwrap()["text"], "A", "Shift still types");
        assert_eq!(key("+").unwrap()["key"], "+");
        assert_eq!(key(" ").unwrap()["code"], "Space");
        assert_eq!(key("ctrl+shift+k").unwrap(), key("Ctrl+Shift+k").unwrap(), "as an app's chords");
        let m = key("mod+s").unwrap()["modifiers"].as_u64();
        assert_eq!(m, Some(if cfg!(target_os = "macos") { 4 } else { 2 }), "mod: the platform's");
        assert!(key("Hyper+a").unwrap_err().contains("is not a modifier"));
        assert!(key("Enterr").unwrap_err().contains("no key is named Enterr"));
    }

    #[cfg(unix)]
    use dioxus::prelude::*;

    #[cfg(unix)]
    fn screen() -> Element {
        rsx! {
            form {
                select { "data-t": "theme",
                    option { value: "light", "Light" }
                    option { value: "dark", "Dark" }
                }
                input { "data-t": "agree", r#type: "checkbox" }
                label { "Email", input { r#type: "email" } }
                input { placeholder: "Title", value: "draft" }
                input {}
                button { "aria-pressed": "true", "Save" }
                button { disabled: true, "Save" }
                button { "aria-label": "", "Buy milk!" }
                button { "ปิด" }
                a { href: "/help", "Help" }
                button { "data-t": "SaveButton3", "the author's" }
                h2 { "A heading that is far too long to be shown here in full, cut" }
                ul { li { "data-t": "row-1", "One thing", button { "Delete" } } li { "Two", button { "Delete" } } }
                div { role: "switch", "aria-checked": "false", "aria-expanded": "true", "Sw" }
            }
            div { style: "display:none", button { "Hidden" } }
        }
    }

    /// The live surface is TestUi's: the same markup, read by a real
    /// Chromium, gives the same lines and names (skipped without one).
    #[cfg(unix)]
    #[test]
    fn the_live_surface_reads_a_page_as_testui_does() {
        let ui = aio::testing::TestUi::new(screen);
        let Some(page) = crate::cdp::tests::Chromium::open(&ui.html()) else { return };
        aio::rt().block_on(async {
            let mut s = page.session().await;
            assert_eq!(read(&mut s, None, false).await.unwrap(), ui.surface());
            let placed = read(&mut s, None, true).await.unwrap();
            assert!(placed.lines().all(|l| l.contains(" @")), "--rects: each where it is: {placed}");
            assert_eq!(names(&mut s).await.unwrap(), ui.names());
            let row = read(&mut s, Some("row-1"), false).await.unwrap();
            assert_eq!(row, "  generic \"One thingDelete\" [row-1]\n    button \"Delete\"", "a path: it, then under it");
            let leaf = read(&mut s, Some("row-1/DeleteButton"), false).await.unwrap();
            assert_eq!(leaf, "  button \"Delete\"", "a leaf is itself, never nothing");
            let e = read(&mut s, Some("nope"), false).await.unwrap_err();
            assert!(
                e.contains("no element with data-t=\"nope\"; the handles there are: SaveButton3, agree, row-1, theme; by name: Form"),
                "{e}"
            );
        });
    }

    /// aio am-trigger: each action, with real input, as the page's own
    /// listeners see it (skipped without Chromium).
    #[cfg(unix)]
    #[test]
    fn a_trigger_drives_the_page_as_a_person_does() {
        let html = include_str!("surface_test.html");
        let Some(page) = crate::cdp::tests::Chromium::open(html) else { return };
        aio::rt().block_on(async {
            let mut s = page.session().await;
            // What the action said, and what the page's listeners logged.
            async fn step(s: &mut Session, p: &str, a: &str, t: Option<&str>) -> (Result<String, String>, Vec<String>) {
                let r = trigger(s, p, a, t).await;
                let l = s.eval("L.splice(0)").await.unwrap();
                (r, l.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect())
            }
            let none = Vec::<String>::new();
            assert_eq!(
                step(&mut s, "SaveButton2", "click", None).await,
                (Ok("click SaveButton2".into()), vec!["save2".into()])
            );
            assert_eq!(step(&mut s, "SaveButton2", "dblclick", None).await.1, ["save2", "save2", "dbl"]);
            assert_eq!(step(&mut s, "row-2/DeleteButton", "click", None).await.1, ["del2"], "a path inside another");
            assert_eq!(step(&mut s, "EmailInput", "type", Some("a@b")).await.1, ["focus"], "no change while typing");
            assert_eq!(step(&mut s, "EmailInput", "type", Some(".c")).await.1, none, "type appends");
            let value = "document.querySelector('input').value";
            assert_eq!(step(&mut s, "EmailInput", "focus", None).await.1, none);
            assert_eq!(s.eval(value).await.unwrap(), "a@b.c");
            assert_eq!(step(&mut s, "EmailInput", "setValue", Some("x")).await.1, none);
            assert_eq!(s.eval(value).await.unwrap(), "x", "setValue replaces");
            assert_eq!(step(&mut s, "EmailInput", "blur", None).await.1, ["change x", "blur"]);
            assert_eq!(step(&mut s, "EmailInput", "clear", None).await.1, ["focus"]);
            assert_eq!(s.eval(value).await.unwrap(), "");
            let to_the_box = step(&mut s, "agree", "check", None).await.1;
            assert_eq!(to_the_box, ["change ", "blur", "agree true"], "the field loses focus to the click");
            assert_eq!(step(&mut s, "agree", "check", None).await.1, none, "checking twice is still checked");
            assert_eq!(step(&mut s, "agree", "uncheck", None).await.1, ["agree false"]);
            let e = step(&mut s, "SaveButton", "check", None).await.0.unwrap_err();
            assert!(e.contains("no checked state: click it instead"), "{e}");
            assert_eq!(step(&mut s, "theme", "select", Some("dark")).await.1, ["theme dark"]);
            let e = step(&mut s, "theme", "select", Some("blue")).await.0.unwrap_err();
            assert!(e.contains("has no option \"blue\"; it has: \"light\", \"dark\", \"neon\""), "{e}");
            assert!(step(&mut s, "theme", "select", Some("neon")).await.0.unwrap_err().contains("is disabled"));
            assert_eq!(step(&mut s, "key", "press", Some("Ctrl+k")).await.1, ["down k ctrl", "up k"]);
            assert_eq!(step(&mut s, "key", "keyDown", Some("Escape")).await.1, ["down Escape"]);
            assert_eq!(step(&mut s, "key", "keyUp", Some("Escape")).await.1, ["up Escape"]);
            assert_eq!(
                step(&mut s, "key", "press", Some("Enter")).await.1,
                ["down Enter", "submit", "up Enter"],
                "Enter submits by the HTML rule"
            );
            assert_eq!(step(&mut s, "window", "press", Some("Escape")).await.1, ["window Escape"], "a global key");
            let e = step(&mut s, "window", "click", None).await.0.unwrap_err();
            assert!(e.contains("the window takes press, keyDown and keyUp"), "{e}");
            assert_eq!(step(&mut s, "pane", "scroll", Some("100")).await.1, ["scroll 100"]);
            assert_eq!(step(&mut s, "from", "dragTo", Some("onto")).await.1, ["grab", "drop"]);
            assert_eq!(step(&mut s, "tip", "hover", None).await.1, ["hover"]);
            let e = step(&mut s, "off", "click", None).await.0.unwrap_err();
            assert!(e.contains("can't click data-t=\"off\": it is disabled"), "{e}");
            let e = step(&mut s, "HiddenButton", "click", None).await.0.unwrap_err();
            assert!(e.contains("by name: Form, EmailInput"), "hidden: no name: {e}");
        });
    }

    #[test]
    fn an_action_is_known_and_takes_a_text_only_when_it_needs_one() {
        let port = crate::cdp::Reach::Port(aio::rt().block_on(crate::cdp::tests::endpoint(Default::default())));
        aio::rt().block_on(async {
            let mut s = crate::cdp::window(&port, 0).await.unwrap();
            assert!(trigger(&mut s, "x", "smash", None).await.unwrap_err().contains("click, dblclick"));
            assert!(trigger(&mut s, "x", "type", None).await.unwrap_err().contains("type needs a text"));
            assert!(trigger(&mut s, "x", "click", Some("y")).await.unwrap_err().contains("takes no text"));
        });
    }

    /// A hidden window (minimized, covered) draws no frames: an action on
    /// it is done and said, never "no answer" (the soak's covered windows).
    #[cfg(unix)]
    #[test]
    fn a_trigger_on_a_hidden_window_is_done_and_answers() {
        let Some(page) = crate::cdp::tests::Chromium::open("<button onclick=\"this.textContent='done'\">Go</button>")
        else {
            return;
        };
        aio::rt().block_on(async {
            let mut s = page.session().await;
            let w = s.call("Browser.getWindowForTarget", json!({})).await.unwrap();
            let min = json!({ "windowId": w["windowId"], "bounds": { "windowState": "minimized" } });
            s.call("Browser.setWindowBounds", min).await.unwrap();
            for _ in 0..100 {
                if s.eval("document.hidden").await.unwrap() == json!(true) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            assert_eq!(s.eval("document.hidden").await.unwrap(), json!(true), "minimized: hidden");
            assert_eq!(trigger(&mut s, "GoButton", "click", None).await, Ok("click GoButton".into()));
            assert_eq!(s.eval("document.querySelector('button').textContent").await.unwrap(), json!("done"));
        });
    }
}

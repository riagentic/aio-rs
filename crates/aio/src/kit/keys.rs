//! App-wide key bindings (aio's `onGlobalKey` and its chord strings): `"mod+s"`,
//! `"ctrl+shift+s"`, `"alt+f1"`, `"ctrl++"`, `"?"`. A chord names a keycap
//! and matches its modifiers exactly (`mod` = Ctrl or ⌘); a binding lives as
//! long as the component that made it. Bindings fire under [`GlobalKeys`],
//! the app root, and not while typing in a field unless they say so; a key
//! pressed with nothing focused (`<body>`) reaches them too (the page's
//! [`KEYS`](crate::page::KEYS)). [`use_key_claim`] takes a chord before
//! anything else hears it (aio's window listener in capture mode).
use crate::ui::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The KeyboardEvent spelling of a key name: `"enter"` → `"Enter"`, `"f5"` →
/// `"F5"`, `"esc"` → `"Escape"`. One character is left alone (`K` ≠ `k`).
pub fn normalize_key(key: &str) -> String {
    if key.chars().count() < 2 {
        return key.into();
    }
    let low = key.to_lowercase();
    let named = match low.as_str() {
        "enter" | "return" => "Enter",
        "esc" | "escape" => "Escape",
        "space" | "spacebar" => " ",
        "tab" => "Tab",
        "backspace" => "Backspace",
        "delete" | "del" => "Delete",
        "insert" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "up" | "arrowup" => "ArrowUp",
        "down" | "arrowdown" => "ArrowDown",
        "left" | "arrowleft" => "ArrowLeft",
        "right" | "arrowright" => "ArrowRight",
        _ => match low.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
            Some(n @ 1..=24) if !low[1..].starts_with('0') => return format!("F{n}"),
            _ => return key.into(),
        },
    };
    named.into()
}

/// A US punctuation keycap: its `code` and what Shift makes it type.
fn us_punct(bind: &str) -> Option<(&'static str, &'static str)> {
    Some(match bind {
        "/" => ("Slash", "?"),
        "=" => ("Equal", "+"),
        "-" => ("Minus", "_"),
        "[" => ("BracketLeft", "{"),
        "]" => ("BracketRight", "}"),
        "\\" => ("Backslash", "|"),
        ";" => ("Semicolon", ":"),
        "'" => ("Quote", "\""),
        "," => ("Comma", "<"),
        "." => ("Period", ">"),
        "`" => ("Backquote", "~"),
        _ => return None,
    })
}

/// Does a keydown (`key`, `code`) press the keycap `bind` names? A digit is
/// its digit-row key (Shift+1 types `!`); a letter is the key typing it, or
/// its US position when the key typed no ASCII (macOS Alt+F types `ƒ`, a
/// non-Latin layout); US punctuation is its position when Shift typed its
/// twin (`?` for `/`) or Alt typed no ASCII; anything else is `key` alone.
pub fn keycap_match(bind: &str, key: &str, code: &str, shift: bool, alt: bool) -> bool {
    if key.to_lowercase() == bind.to_lowercase() {
        return true;
    }
    let one = |s: &str| {
        let mut cs = s.chars();
        cs.next().filter(|_| cs.next().is_none())
    };
    if let Some(d) = one(bind).filter(char::is_ascii_digit) {
        return code == format!("Digit{d}");
    }
    let no_ascii = key == "Dead" || key == "Unidentified" || one(key).is_some_and(|c| !c.is_ascii());
    if let Some((at, twin)) = us_punct(bind) {
        return code == at && ((shift && key == twin) || (alt && no_ascii));
    }
    one(bind).is_some_and(|c| c.is_ascii_alphabetic()) && no_ascii && code == format!("Key{}", bind.to_uppercase())
}

/// A parsed binding: the keycap, the modifiers it names, and how it matches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chord {
    pub key: String,
    pub ctrl: bool,
    pub meta: bool,
    pub alt: bool,
    pub shift: bool,
    /// Ctrl or ⌘, whichever the platform's shortcuts use.
    pub cmd: bool,
    /// Unnamed modifiers must be up (the default); `false`: don't care.
    pub exact: bool,
    /// Fires while typing in a field too.
    pub in_fields: bool,
    /// Fires again while its key is held (the key's auto-repeat): by
    /// default only movement does, the arrows, PageUp/PageDown and
    /// Backspace (aio's `REPEATABLE_KEYS`), so a held shortcut fires once.
    pub repeats: bool,
}

/// The keys whose auto-repeat fires a binding unless it says otherwise.
const REPEATABLE: [&str; 7] = ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "PageUp", "PageDown", "Backspace"];

/// How a chord matched: naming the Shift state, or leaving it free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Set,
    Free,
}

impl Chord {
    /// `"ctrl+s"`, `"Ctrl+Shift+S"`, `"mod+k"`, `"cmd+option+esc"`, `"ctrl++"`:
    /// modifiers (`ctrl`/`control`, `meta`/`cmd`/`command`/`super`,
    /// `alt`/`option`, `shift`, `mod`), then one key. A word before the key
    /// that is no modifier, or no key, is an error.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let fail = |why: String| {
            format!(
                "key chord {spec:?}: {why}: write modifier+…+key with ctrl/shift/alt/meta/mod (\"ctrl+s\", \"mod+shift+k\", \"alt+f1\")"
            )
        };
        let (mods, key) = match spec.strip_suffix("++") {
            _ if spec.chars().count() == 1 => ("", spec),
            Some(m) => (m, "+"),
            None => spec.rsplit_once('+').unwrap_or(("", spec)),
        };
        if key.is_empty() {
            return Err(fail("no key after the last +".into()));
        }
        let key = normalize_key(key);
        let repeats = REPEATABLE.contains(&key.as_str());
        let mut c = Self { key, exact: true, repeats, ..Self::default() };
        for m in mods.split('+').filter(|_| !mods.is_empty()) {
            match m.to_lowercase().as_str() {
                "ctrl" | "control" => c.ctrl = true,
                "cmd" | "command" | "meta" | "super" => c.meta = true,
                "alt" | "option" => c.alt = true,
                "shift" => c.shift = true,
                "mod" => c.cmd = true,
                _ => return Err(fail(format!("{m:?} is not a modifier"))),
            }
        }
        Ok(c)
    }

    /// Unnamed modifiers don't matter.
    pub fn loose(self) -> Self {
        Self { exact: false, ..self }
    }

    /// Fires while typing in a field too.
    pub fn in_fields(self) -> Self {
        Self { in_fields: true, ..self }
    }

    /// Fires again while its key is held (`true`), or only once (`false`).
    pub fn repeating(self, repeats: bool) -> Self {
        Self { repeats, ..self }
    }

    /// Does a keydown fire it? `None` no.
    pub fn hit(&self, key: &str, code: &str, mods: Modifiers) -> Option<Hit> {
        if !keycap_match(&self.key, key, code, self.shift, self.alt) {
            return None;
        }
        // Exact: an unnamed modifier must be up, except Ctrl/⌘ under `mod`
        // and Shift on a symbol key (Shift is how "?" is typed).
        let want = |named: bool, free: bool| match (named, self.exact && !free) {
            (true, _) => Some(true),
            (false, true) => Some(false),
            (false, false) => None,
        };
        let symbol = {
            let mut cs = self.key.chars();
            cs.next()
                .is_some_and(|c| cs.next().is_none() && c.to_lowercase().eq(c.to_uppercase()) && !c.is_ascii_digit())
        };
        let shift = want(self.shift, symbol);
        let checks = [
            (want(self.ctrl, self.cmd), Modifiers::CONTROL),
            (want(self.meta, self.cmd), Modifiers::META),
            (want(self.alt, false), Modifiers::ALT),
            (shift, Modifiers::SHIFT),
        ];
        if checks.iter().any(|(w, m)| w.is_some_and(|w| mods.contains(*m) != w)) {
            return None;
        }
        if self.cmd && !mods.intersects(Modifiers::CONTROL | Modifiers::META) {
            return None;
        }
        Some(if shift.is_some() { Hit::Set } else { Hit::Free })
    }
}

thread_local! {
    /// The key a field saw on its way up to the root, this keystroke.
    static FROM_FIELD: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Put on a plain `input`'s `onkeydown` (the kit's fields do it already):
/// global bindings then leave the keystroke to the field.
pub fn field_key(e: &KeyboardEvent) {
    FROM_FIELD.with(|f| *f.borrow_mut() = Some(e.key().to_string()));
}

struct Binding {
    id: u64,
    chord: Chord,
    run: Callback<KeyboardEvent>,
}

#[derive(Clone, Default)]
struct Keys {
    all: Rc<RefCell<Vec<Binding>>>,
    next: Rc<Cell<u64>>,
}

impl Keys {
    /// One keystroke: the bindings it fires (on a Shift-held press, one
    /// naming Shift beats one leaving it free). A keystroke a binding took,
    /// or left to a field, goes no further (`true`).
    fn fire(&self, e: KeyboardEvent) -> bool {
        let key = e.key().to_string();
        let typing = FROM_FIELD.with(|f| f.borrow_mut().take()).is_some_and(|k| k == key);
        let (code, mods, again) = (e.code().to_string(), e.modifiers(), e.is_auto_repeating());
        let (runs, matched): (Vec<Callback<KeyboardEvent>>, bool) = {
            let all = self.all.borrow();
            let hits: Vec<_> = all.iter().filter_map(|b| Some((b, b.chord.hit(&key, &code, mods)?))).collect();
            // A held key's repeat fires only the bindings that repeat; the
            // others take it all the same (nothing else hears it).
            let taken = |b: &Binding| (!typing || b.chord.in_fields) && (!again || b.chord.repeats);
            let set = hits.iter().any(|(b, h)| *h == Hit::Set && taken(b));
            let runs = hits
                .iter()
                .filter(|(b, h)| taken(b) && !(*h == Hit::Free && mods.contains(Modifiers::SHIFT) && set))
                .map(|(b, _)| b.run)
                .collect();
            (runs, !hits.is_empty())
        };
        if matched {
            e.stop_propagation();
        }
        for r in runs {
            r.call(e.clone());
        }
        matched || typing
    }
}

/// The claims ([`use_key_claim`]): bindings of their own, and whether any is
/// mounted (the element the page tells them to is there only then).
#[derive(Clone, Copy)]
struct Claims {
    keys: Signal<Keys>,
    any: Signal<bool>,
}

impl Claims {
    /// The page's copy of the claimed chords, and the element for them.
    fn changed(self) {
        let all = self.keys.read_untracked().all.clone();
        let chords: Vec<serde_json::Value> = all
            .borrow()
            .iter()
            .map(|b| {
                let c = &b.chord;
                serde_json::json!({ "key": c.key, "ctrl": c.ctrl, "meta": c.meta, "alt": c.alt, "shift": c.shift,
                    "cmd": c.cmd, "exact": c.exact, "in_fields": c.in_fields })
            })
            .collect();
        _ = eval(&format!("window.__aioClaims = {}", serde_json::Value::from(chords)));
        let any = !all.borrow().is_empty();
        if *self.any.read_untracked() != any {
            self.any.set(any);
        }
    }
}

/// The page's half of [`use_key_claim`] (installed once, by [`GlobalKeys`]):
/// a claimed key is stopped in the window's capture phase, before any
/// element hears it, and told to the claims' element alone. `hits` is
/// [`Chord::hit`] by the key's name (the Rust side also matches by the
/// key's place).
const PAGE_KEYS: &str = r#"(() => {
  if (window.__aioKeys) return;
  window.__aioKeys = true;
  window.__aioClaims = window.__aioClaims || [];
  const root = () => document.querySelector('[data-aio-keys]');
  const copy = (e, on) => on.dispatchEvent(new KeyboardEvent(e.type, e));
  const field = (t) => t instanceof Element && (t.matches('input, textarea, select') || t.isContentEditable);
  const hits = (c, e) => {
    if ((!c.in_fields && field(e.target)) || e.key.toLowerCase() !== c.key.toLowerCase()) return false;
    const symbol = [...c.key].length === 1 && c.key.toLowerCase() === c.key.toUpperCase() && !/\d/.test(c.key);
    const want = (named, free) => named ? true : c.exact && !free ? false : null;
    const mods = [[want(c.ctrl, c.cmd), e.ctrlKey], [want(c.meta, c.cmd), e.metaKey], [want(c.alt, false), e.altKey], [want(c.shift, symbol), e.shiftKey]];
    return !mods.some(([w, on]) => w !== null && w !== on) && (!c.cmd || e.ctrlKey || e.metaKey);
  };
  addEventListener('keydown', (e) => {
    const at = root()?.querySelector('[data-aio-claimed]');
    if (!e.isTrusted || !at || !window.__aioClaims.some((c) => hits(c, e))) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    copy(e, at);
  }, true);
  // The pointer's moves reach the window's listeners at most once per
  // MOVE_MS, the last one too (aio throttled its idle re-arm): not every
  // move crosses to the app.
  let last = 0, timer = 0, move = null;
  const send = () => {
    timer = 0;
    last = performance.now();
    const at = root()?.querySelector('[data-aio-moves]');
    if (at && move) at.dispatchEvent(new MouseEvent('mousemove', move));
    move = null;
  };
  addEventListener('mousemove', (e) => {
    if (!e.isTrusted) return;
    move = e;
    if (timer) return;
    const left = MOVE_MS - (performance.now() - last);
    if (left <= 0) send(); else timer = setTimeout(send, left);
  }, { capture: true, passive: true });
})()"#;

/// How often at most the pointer's moves reach the window's listeners
/// ([`WindowEvent::MouseMove`](super::WindowEvent)), the last one too.
pub const MOVE_MS: u32 = 100;

/// The app root that global bindings and window listeners
/// ([`use_window_event`](super::use_window_event)) listen on: put it around
/// the app once; it fills the window. It takes focus when nothing inside
/// has it, so keys reach it from the start; a key pressed while nothing
/// has focus (on `<body>`) reaches it too (the page's [`KEYS`](crate::page::KEYS)).
#[component]
pub fn GlobalKeys(#[props(extends = GlobalAttributes)] attributes: Vec<Attribute>, children: View) -> View {
    use super::window::{Listeners, WindowEvent as W};
    let keys = provide(Keys::default);
    let win = provide(Listeners::default);
    let claims = provide(|| Claims { keys: Signal::new(Keys::default()), any: Signal::new(false) });
    let tell = callback(move |e| win.tell(e));
    rsx! {
        div {
            class: "aio-keys",
            "data-aio-keys": "",
            tabindex: "-1",
            onmounted: move |e| async move {
                _ = eval(&PAGE_KEYS.replace("MOVE_MS", &MOVE_MS.to_string()));
                _ = e.set_focus(true).await
            },
            onkeydown: move |e| {
                if !keys.fire(e.clone()) {
                    tell(W::KeyDown(e))
                }
            },
            onkeyup: move |e| tell(W::KeyUp(e)),
            onmousedown: move |e| tell(W::MouseDown(e)),
            onmouseup: move |e| tell(W::MouseUp(e)),
            onclick: move |e| tell(W::Click(e)),
            onwheel: move |e| tell(W::Wheel(e)),
            onresize: move |e: ResizeEvent| {
                if let Ok(s) = e.get_border_box_size() {
                    tell(W::Resize { width: s.width, height: s.height })
                }
            },
            ..attributes,
            // The page tells the pointer's moves here, MOVE_MS apart.
            div { "data-aio-moves": "", hidden: true, onmousemove: move |e| tell(W::MouseMove(e)) }
            if (claims.any).get() {
                div {
                    "data-aio-claimed": "",
                    hidden: true,
                    onkeydown: move |e| {
                        // Claimed: nothing else hears it, the root's bindings neither.
                        e.stop_propagation();
                        let k = claims.keys.read_untracked().clone();
                        k.fire(e);
                    },
                }
            }
            {children}
        }
    }
}

/// Take `chord` before anything else hears it, wherever the focus is (a
/// dialog's field, a button, nothing), while this component is mounted
/// (aio's window listener in capture mode that stops the key): `f` gets it,
/// and no element's handler, binding or browser default does. Not while
/// typing in a field unless the chord says so ([`Chord::in_fields`]). Mount
/// it only while it should take the key (a component under an `if`). Needs
/// a [`GlobalKeys`] around the app.
pub fn use_key_claim(chord: impl Into<ChordSpec>, f: impl FnMut(KeyboardEvent) + 'static) {
    let chord = match chord.into() {
        ChordSpec(Ok(c)) => c,
        ChordSpec(Err(e)) => panic!("{e}"),
    };
    let run = callback(f);
    let claims = try_context::<Claims>().expect("use_key_claim needs a GlobalKeys around the app");
    let keys = claims.keys.read_untracked().clone();
    let id = once(|| {
        let id = keys.next.get();
        keys.next.set(id + 1);
        keys.all.borrow_mut().push(Binding { id, chord: chord.clone(), run });
        id
    });
    // Told once it rendered (the claims' element is the root's to render).
    effect(move || claims.changed());
    let moved = keys.all.borrow_mut().iter_mut().find(|b| b.id == id).is_some_and(|b| {
        let moved = b.chord != chord;
        b.chord = chord;
        moved
    });
    if moved {
        claims.changed();
    }
    on_cleanup(move || {
        claims.keys.read_untracked().all.borrow_mut().retain(|b| b.id != id);
        claims.changed();
    });
}

/// Run `f` on `chord` (a spec, or a [`Chord`]) while this component is
/// mounted; the latest `f` runs. Needs a [`GlobalKeys`] above it. A bad
/// spec panics: it is the app's code, not its data.
pub fn use_global_key(chord: impl Into<ChordSpec>, f: impl FnMut(KeyboardEvent) + 'static) {
    let chord = match chord.into() {
        ChordSpec(Ok(c)) => c,
        ChordSpec(Err(e)) => panic!("{e}"),
    };
    let run = callback(f);
    let keys = try_context::<Keys>().expect("use_global_key needs a GlobalKeys around the app");
    let id = once(|| {
        let id = keys.next.get();
        keys.next.set(id + 1);
        keys.all.borrow_mut().push(Binding { id, chord: chord.clone(), run });
        id
    });
    if let Some(b) = keys.all.borrow_mut().iter_mut().find(|b| b.id == id) {
        b.chord = chord;
    }
    on_cleanup(move || keys.all.borrow_mut().retain(|b| b.id != id));
}

/// A chord as [`use_global_key`] and [`use_key_claim`] take it: a `&str`
/// ([`Chord::parse`]) or a [`Chord`]. The words, case-insensitive, joined
/// by `+`: modifiers first — `ctrl` (`control`), `meta` (`cmd`, `command`,
/// `super`), `alt` (`option`), `shift`, and `mod` (Ctrl, or ⌘ on macOS: a
/// shortcut's usual key) — then one key: a character (`k`, `/`, `+` as in
/// `ctrl++`) or a DOM key name (`Enter`/`return`, `Escape`/`esc`, `space`,
/// `Tab`, `Backspace`, `Delete`/`del`, `Insert`, `Home`, `End`, `PageUp`,
/// `PageDown`, `ArrowUp`/`up` and the other arrows, `F1`…`F24`). Modifiers
/// not named must be up ([`Chord::loose`] lets them be). `"mod+k"`,
/// `"ctrl+shift+s"`, `"alt+f4"`, `"?"`. `ax trigger <app> window press
/// <chord>` and `TestUi::press` take the same words.
pub struct ChordSpec(Result<Chord, String>);

impl From<&str> for ChordSpec {
    fn from(s: &str) -> Self {
        Self(Chord::parse(s))
    }
}

impl From<Chord> for ChordSpec {
    fn from(c: Chord) -> Self {
        Self(Ok(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(spec: &str) -> Chord {
        Chord::parse(spec).unwrap()
    }

    #[test]
    fn chords_parse_into_a_key_and_its_modifiers() {
        let ctrl = |key: &str| Chord { key: key.into(), ctrl: true, exact: true, ..Chord::default() };
        assert_eq!(c("ctrl+s"), ctrl("s"));
        assert_eq!(c("Ctrl+Shift+S"), Chord { shift: true, ..ctrl("S") });
        assert_eq!(c("mod+k"), Chord { key: "k".into(), cmd: true, exact: true, ..Chord::default() });
        assert_eq!(c("alt+f1").key, "F1");
        let esc = c("cmd+option+esc");
        assert!(esc.meta && esc.alt && esc.key == "Escape");
        assert_eq!(c("ctrl++"), ctrl("+"));
        assert_eq!(c("+").key, "+");
        assert_eq!(c("?").key, "?");
        assert!(Chord::parse("hyper+k").unwrap_err().contains("\"hyper\" is not a modifier"));
        assert!(Chord::parse("ctrl+").unwrap_err().contains("no key after the last +"));
    }

    #[test]
    fn key_names_take_their_keyboard_event_spelling() {
        assert_eq!(normalize_key("enter"), "Enter");
        assert_eq!(normalize_key("esc"), "Escape");
        assert_eq!(normalize_key("space"), " ");
        assert_eq!(normalize_key("f5"), "F5");
        assert_eq!(normalize_key("f25"), "f25");
        assert_eq!(normalize_key("K"), "K");
        assert_eq!(normalize_key("left"), "ArrowLeft");
    }

    const CTRL: Modifiers = Modifiers::CONTROL;
    const SHIFT: Modifiers = Modifiers::SHIFT;

    #[test]
    fn a_chord_matches_its_modifiers_exactly() {
        let save = c("ctrl+s");
        assert_eq!(save.hit("s", "KeyS", CTRL), Some(Hit::Set), "exact: Shift is named up");
        for m in [Modifiers::empty(), CTRL | Modifiers::ALT, Modifiers::META] {
            assert_eq!(save.hit("s", "KeyS", m), None, "{m:?}");
        }
        assert_eq!(c("ctrl+shift+s").hit("S", "KeyS", CTRL | SHIFT), Some(Hit::Set), "the key matches in any case");
        let palette = c("mod+k");
        assert!(palette.hit("k", "KeyK", CTRL).is_some() && palette.hit("k", "KeyK", Modifiers::META).is_some());
        assert!(
            palette.hit("k", "KeyK", CTRL | SHIFT).is_none() && palette.hit("k", "KeyK", Modifiers::empty()).is_none()
        );
        assert!(c("alt+f1").hit("F1", "F1", Modifiers::ALT).is_some());
        assert!(c("alt+f1").hit("F1", "F1", Modifiers::ALT | CTRL).is_none());
        let zoom = c("ctrl++");
        assert_eq!(zoom.hit("+", "Equal", CTRL | SHIFT), Some(Hit::Free), "Shift types +: free");
        assert!(c("ctrl+?").hit("?", "Slash", CTRL | SHIFT).is_some());
        assert!(c("mod+e").loose().hit("E", "KeyE", CTRL | SHIFT).is_some(), "loose: Shift doesn't matter");
    }

    #[test]
    fn a_chord_names_a_keycap_not_the_character_it_types() {
        assert!(c("ctrl+shift+1").hit("!", "Digit1", CTRL | SHIFT).is_some(), "Shift+1 types !");
        assert!(c("ctrl+1").hit("&", "Digit1", CTRL).is_some(), "AZERTY digit row");
        assert!(c("ctrl+1").hit("!", "Digit1", CTRL | SHIFT).is_none(), "Shift is a real modifier on a digit");
        assert!(c("alt+f").hit("ƒ", "KeyF", Modifiers::ALT).is_some(), "macOS Alt+F types ƒ");
        assert!(c("ctrl+a").hit("ф", "KeyA", CTRL).is_some(), "a non-Latin layout");
        assert!(c("ctrl+m").hit(",", "KeyM", CTRL).is_none(), "an ASCII key is never re-read");
        assert!(c("ctrl+shift+/").hit("?", "Slash", CTRL | SHIFT).is_some());
        assert!(c("alt+/").hit("÷", "Slash", Modifiers::ALT).is_some());
        assert!(!keycap_match("/", "!", "Slash", true, false), "AZERTY's Slash key types !");
    }
}

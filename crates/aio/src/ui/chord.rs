//! Key chords as bindings name them (aio's chord strings): `"mod+s"`,
//! `"ctrl+shift+s"`, `"alt+f1"`, `"ctrl++"`, `"?"`, parsed and matched
//! (pure, renderer-free: the kit's bindings and `TestUi` share them).
use crate::ui::prelude::Modifiers;

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

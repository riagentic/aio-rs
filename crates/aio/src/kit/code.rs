//! A code editor's light core: a real `textarea` over a highlighted copy
//! of its text, in one scroller. The app gives the highlighting (runs of
//! class and text per line: the kit has no grammars), the line marks and
//! the keys it wants; the page does Tab, so typing never waits on the app.
//! Its sheet and script come with it: an app without one pays nothing.
use super::join;
use crate::ui::prelude::*;
use std::hash::{Hash, Hasher};

/// The sheet, put in with the component (no rule in the kit's [`super::CSS`]).
const CSS: &str = crate::shrunk!(css, include_str!("code.css"));
/// The textarea's keys and IME, in the page (`el`).
const SCRIPT: &str = crate::shrunk!(js, include_str!("code.js"));

/// One line as drawn: its key in the list, its marks' classes, its runs.
#[derive(Clone, Debug, PartialEq)]
struct Line {
    key: String,
    mark: String,
    runs: Vec<(String, String)>,
}

/// The lines of `text` as drawn: line `i` in `spans[i]`'s runs when they
/// spell it (else plain: highlighting that lags the text never misplaces
/// it), with the classes `marks` give line `i`. Keyed by content (and how
/// many lines alike came before), so a line put in or taken out leaves
/// the others' rows be.
fn lines(text: &str, spans: &[Vec<(String, String)>], marks: &[(usize, String)]) -> Vec<Line> {
    let mut seen = std::collections::HashMap::<u64, u32>::new();
    text.split('\n')
        .enumerate()
        .map(|(i, line)| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let runs = match spans.get(i) {
                Some(r) if spells(r, line) => r.iter().filter(|(_, t)| !t.is_empty()).cloned().collect(),
                _ => vec![(String::new(), line.to_owned())],
            };
            let mark = marks.iter().filter(|(l, _)| *l == i).map(|(_, c)| c.as_str()).collect::<Vec<_>>().join(" ");
            let mut h = std::hash::DefaultHasher::new();
            (&runs, &mark).hash(&mut h);
            let h = h.finish();
            let n = seen.entry(h).or_default();
            *n += 1;
            Line { key: format!("{h:x}.{n}"), mark, runs }
        })
        .collect()
}

/// `runs`' texts, one after another, are `line`.
fn spells(runs: &[(String, String)], line: &str) -> bool {
    let mut rest = line;
    runs.iter().all(|(_, t)| rest.strip_prefix(t.as_str()).map(|r| rest = r).is_some()) && rest.is_empty()
}

/// A code editor's core (aio-rs's own): monospaced text with line numbers,
/// the app's highlighting and line marks, Tab that indents. The `textarea`
/// is the real control (name it with a [`Field`](super::Field) around it
/// or `aria-label`); a copy of its text under it shows the colours.
///
/// - `value`, `oninput(text)`: the text, controlled as [`Textarea`](super::Textarea)'s.
/// - `spans`: per line, its runs of `(class, text)`; a line whose runs
///   don't spell it (highlighting not caught up yet) shows plain.
/// - `marks`: `(line, class)`, lines from 0: the class goes on the line's
///   row (number and text); `error`, `warning` and `current` have a look.
/// - `tab_size` (4): Tab puts spaces to the next stop, or indents every line
///   of a selection; Shift+Tab outdents; Escape, then Tab, leaves the field.
///   Done in the page: undone by Ctrl+Z, never late.
/// - `wrap`: long lines wrap (off: the box scrolls sideways).
/// - `onkeydown`: the field's keys (Ctrl+Enter to run).
/// - `onmounted`: the textarea's handle, for its selection
///   ([`MountedData::selection`], `set_selection`, `insert_text`).
/// - `class` and `style` size the box (it scrolls inside); every other
///   attribute (`id`, `aria-*`, `readonly`, `placeholder`, `"data-t"`)
///   goes on the textarea.
#[component]
pub fn CodeArea(
    #[props(into)] value: Prop<String>,
    oninput: Option<EventHandler<String>>,
    #[props(default, into)] spans: Prop<Vec<Vec<(String, String)>>>,
    #[props(default, into)] marks: Prop<Vec<(usize, String)>>,
    #[props(default = 4)] tab_size: Prop<u32>,
    #[props(default)] wrap: Prop<bool>,
    #[props(default, into)] class: Prop<String>,
    onkeydown: Option<EventHandler<KeyboardEvent>>,
    onmounted: Option<EventHandler<MountedEvent>>,
    #[props(extends = GlobalAttributes)] attributes: Vec<Attribute>,
) -> View {
    let w = super::form::use_field(false, false);
    let (outer, attributes): (Vec<_>, Vec<_>) = attributes.into_iter().partition(|a| a.name == "style");
    // What the field holds: the app's value, and what was typed before the
    // app answered (or with no `oninput` at all).
    let text = signal(move || value.get_untracked());
    effect(move || text.set(value.get()));
    let rows = memo(move || text.with(|t| spans.with(|s| marks.with(|m| lines(t, s, m)))));
    let digits = memo(move || rows.with(Vec::len).to_string().len().max(2));
    let tab = memo(move || tab_size.get().clamp(1, 16));
    rsx! {
        div {
            class: join(if wrap.get() { "aio-code aio-code--wrap" } else { "aio-code" }, &class.get()),
            style: "--aio-code-digits: {digits}; tab-size: {tab}",
            translate: "no",
            ..outer,
            style { {CSS} }
            div { class: "aio-code__doc",
                textarea {
                    class: "aio-code__in",
                    value: value.get(),
                    wrap: if wrap.get() { "soft" } else { "off" },
                    spellcheck: "false",
                    autocomplete: "off",
                    autocapitalize: "off",
                    "data-tab": "{tab}",
                    id: w.id.clone(),
                    "aria-describedby": w.described(),
                    "aria-invalid": w.invalid().then_some("true"),
                    "aria-required": w.required(),
                    oninput: move |e| {
                        text.set(e.value());
                        if let Some(f) = oninput {
                            f.call(e.value())
                        }
                    },
                    onkeydown: move |e| if let Some(f) = onkeydown { f.call(e) },
                    onmounted: move |e| {
                        e.data().run(SCRIPT);
                        if let Some(f) = onmounted {
                            f.call(e)
                        }
                    },
                    ..attributes,
                }
                div { class: "aio-code__lines", "aria-hidden": "true",
                    for l in rows.get() {
                        div { key: "{l.key}", class: join("aio-code__line", &l.mark),
                            span { class: "aio-code__text",
                                for (i, (c, t)) in l.runs.into_iter().enumerate() {
                                    span { key: "{i}", class: (!c.is_empty()).then_some(c), "{t}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(r: &[(&str, &str)]) -> Vec<(String, String)> {
        r.iter().map(|(c, t)| (c.to_string(), t.to_string())).collect()
    }

    #[test]
    fn a_line_shows_its_runs_only_when_they_spell_it() {
        let spans = vec![runs(&[("kw", "let"), ("", " x"), ("", "")]), runs(&[("kw", "fn")])];
        let got = lines("let x\nfn main\r\n", &spans, &[]);
        assert_eq!(got.len(), 3, "the last line, empty, is a line");
        assert_eq!(got[0].runs, runs(&[("kw", "let"), ("", " x")]), "empty runs drawn as nothing");
        assert_eq!(got[1].runs, runs(&[("", "fn main")]), "runs that lag the text: plain, a CR dropped");
        assert_eq!(got[2].runs, runs(&[("", "")]));
        assert!(spells(&[], "") && !spells(&runs(&[("", "ab")]), "a") && !spells(&runs(&[("", "a")]), "ab"));
    }

    #[test]
    fn marks_go_on_their_lines_and_alike_lines_keep_apart_keys() {
        let got = lines("a\n\na\n", &[], &[(0, "error".into()), (0, "current".into()), (9, "x".into())]);
        assert_eq!(got.iter().map(|l| l.mark.as_str()).collect::<Vec<_>>(), ["error current", "", "", ""]);
        let keys: std::collections::HashSet<_> = got.iter().map(|l| &l.key).collect();
        assert_eq!(keys.len(), 4, "unique keys: {got:?}");
        let moved = lines("b\na\n\na\n", &[], &[]);
        assert_eq!(moved[2].key, got[1].key, "a line put in above leaves the rows below their keys");
    }
}

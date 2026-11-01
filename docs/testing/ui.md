# Testing views

`aio::testing::TestUi` mounts the app's real views in a hidden page of the
pinned Electron runtime, the engine the app's window runs, with no window
on any screen. Layout, CSS, focus, forms and scrolling are the browser's
own. The test drives the page as a person does: it finds elements by what
a person sees (a label and a role) or by a `data-t` handle, moves the
mouse, presses keys, and reads what shows. The views run in the test's
thread and talk to the page over the same wire the window uses.

`ax setup` fetches the runtime once (`AIORT_RUNTIME` points at another
runtime folder). `TestUi` needs the `kit` feature, which an app made by
`ax create` has.

## A first test

```rust,no_run
use aio::act;
use aio::testing::TestUi;
use dioxus::prelude::*;

#[aio::cell(persist, id = "todo")]
pub struct Todo {
    pub items: Vec<String>,
}

#[aio::methods]
impl Todo {
    pub fn add(&mut self, text: String) -> aio::Result {
        aio::ensure!(!text.trim().is_empty(), "say what to do");
        self.items.push(text);
        Ok(())
    }
}

aio::client!(Todo, todo);

#[component]
fn App() -> Element {
    use_live();
    let items = part(|t| &t.items);
    let mut draft = use_signal(String::new);
    rsx! {
        form {
            onsubmit: move |e| {
                e.prevent_default();
                if act!(add(draft())).is_some() {
                    draft.set(String::new());
                }
            },
            input { placeholder: "What to do", value: "{draft}", oninput: move |e| draft.set(e.value()) }
            button { r#type: "submit", "Add" }
        }
        ul { "data-t": "list",
            for item in items.iter() {
                li { "{item}" }
            }
        }
        p { "data-t": "count", "{items.len()} to do" }
    }
}

# fn main() {
let mut ui = TestUi::new(App);
ui.type_text("WhatToDoInput", "buy milk");
ui.click("AddButton");
assert_eq!(ui.text("count"), "1 to do");
assert_eq!(ui.value("WhatToDoInput"), "", "the field was cleared");

ui.set_value("WhatToDoInput", "call Bo");
ui.press("WhatToDoInput", "Enter"); // Enter in a form's field submits it
assert_eq!(ui.text("list/CallBoItem"), "call Bo"); // a list item is named by its text
assert_eq!(HANDLE.peek().state().items.len(), 2, "the cell, not only the page");

// An empty add is refused: take the refusal, or it fails the test at its end.
ui.click("AddButton");
assert_eq!(ui.failures(), ["add: say what to do"]);
# }
```

- `WhatToDoInput` and `AddButton` are automatic names: the accessible
  name, then the role, as one word. No `data-t` was needed.
- Each action settles before it returns: the views rendered, the page
  drew, its events came back. There is nothing to `await` and no sleep.
- `HANDLE` is the source `aio::client!` made for the app's cell: read it
  to assert on the cell beside the page.

## Mounting

| Call | Mounts |
|---|---|
| `TestUi::new(App)` | the app at 1180×6000 px, light, scrollbars shown |
| `TestUi::with(Card, CardProps { .. })` | one component alone with its props (aio's `testComponent`) |
| `Mount::default().size(390, 844).app(App)` | the app as the options below say |
| `Mount::default().dark().component(Card, props)` | one component, with options |

| `Mount` option | Does |
|---|---|
| `.size(w, h)` | the window's size in CSS px (a phone: `390, 844`); the default is tall, so every pane fits without scrolling |
| `.dark()` | `prefers-color-scheme: dark` |
| `.no_scrollbars()` | no scrollbars and no room kept for them: pixel-stable shots on every OS |
| `.before(\|\| ..)` | runs in the app's root scope before its first render: a seed, a route, a global; repeatable |
| `.network(&["api.example.org"])` | hosts the page may reach besides this machine (`"*"`: any) |

Each mount is a fresh app instance: its `GlobalSignal`s, its `client!`
source and its context start over, so tests share nothing through them.
Each mount also gets its own sandbox (see [the sandbox](README.md#every-test-runs-in-a-sandbox)).
Every mount runs a contrast audit and prints, never fails, the text a
person can't read (`aio: text a person can't read: …`); assert on
`ui.contrast()` to make it a failure.

## Finding elements

Every method takes `t`, which names exactly one showing element:

| `t` | Finds |
|---|---|
| `count` | the element with `"data-t": "count"` |
| `SaveButton` | the auto name: accessible name + role, see below |
| `SaveButton2` | the second element with that auto name, in document order |
| `list/MilkItem` | `MilkItem` inside the element `list` (any depth; paths nest) |
| `text=Skip this version` | the one element showing exactly that text (spaces folded); the button around it when it is the button's whole text |
| `css=.rows li:nth-child(2)` | the one element a CSS selector matches; `list/css=li` inside `list` |
| `window` | for `focus` and keys only: no element, keys go to the document |

Two matches for `text=` or `css=` fail, naming them: a test never acts
on a guess. A `data-t` handle is checked first, then the auto names.

### Auto names

An element's name is its accessible name, then its role, PascalCased
into one word (five words and 32 characters at most):

| Element | Name |
|---|---|
| `button { "Save" }` | `SaveButton` |
| `button { "aria-label": "Close", "×" }` | `CloseButton` |
| `input { placeholder: "Email" }` | `EmailInput` |
| `label { r#for: "e", "Email" } input { id: "e" }` | `EmailInput` |
| `label { "Remember me" input { r#type: "checkbox" } }` | `RememberMeCheckbox` |
| `input { r#type: "radio", "aria-label": "Small" }` | `SmallRadio` |
| `select { "aria-label": "Size" }` | `SizeSelect` |
| `a { href: "/help", "Help" }` | `HelpLink` |
| `h2 { "Settings" }` | `SettingsHeading` |
| `img { alt: "Logo" }` | `LogoImg` |
| `li { "Milk" }`, `tr { .. }`, `form { .. }` | `MilkItem`, `Row`, `Form` (their own text, not their controls') |
| `div { role: "switch", "aria-label": "Wi-Fi" }` | `WiFiSwitch` |
| a field with no label | `Input`, then `Input2` |

- The accessible name is, in order: `aria-label`, the `<label for>`, a
  wrapping `<label>`, the element's own text (not for a field), its
  `placeholder`, its `alt`.
- The role is its `role` attribute, else the one its tag implies
  (`button`, `link`, `checkbox`, `radio`, `slider`, `textbox` (written
  `Input`), `combobox` (written `Select`), `heading`, `img`, `dialog`).
- A name that already ends with its role isn't doubled: `"Send button"`
  is `SendButton`.
- A plain `div` or `span` has no auto name: give it a `data-t` when a test
  must read it. A handle is also the stable choice where the visible copy
  will change (translations, marketing text).
- `data-t` is an ordinary attribute: it stays in the HTML, a CSS selector
  can match it, and `ax surface` shows it as `[handle]`.

`ui.names()` lists every auto name in document order, `ui.handles()`
every `data-t` (sorted, showing or not), and `ui.surface()` both, nested.
A name that misses fails, listing the handles and names that are there:
`no element with data-t="advise"; the handles there are: …; by name: …`.
A miss that looks like a selector says how to write one
(`(a CSS selector? css=.row)`).

### Showing means showing

`present(t)` is true when the element is in the document and not hidden
by CSS: `display: none` on it or on anything around it, the `hidden`
attribute, `visibility: hidden`. `absent(t)` is its opposite,
`has_handle(t)` asks for a `data-t` alone (no auto names), `count(css)`
counts selector matches. Every action refuses a hidden element, saying
why: `data-t="inside" can't be used: it is hidden (display: none on an
enclosing <div>)`.

## Acting

Each action is what a person's hand does, sent as real input over the
DevTools protocol, so the page's own rules apply. Then it settles.

| Action | Does |
|---|---|
| `click(t)` | moves the mouse to the centre of what shows of it, presses, releases (pointer and mouse events, then `click`); a submit button submits its form |
| `click_with(t, Modifiers::SHIFT)` | a click holding keys; on macOS Ctrl+click is the context click, use `Modifiers::META` for "Ctrl" there |
| `click_at(t, fx, fy)` | a click at a fraction of its box (`0.0, 0.0` top left) |
| `dblclick(t)` | two clicks, the second also a `dblclick` |
| `hover(t)` | the mouse over its centre (`:hover` shows, until the next action) |
| `drag_by(t, dx, dy)` | press at its centre, move by `(dx, dy)` px, release |
| `drag_along(t, &[(dx, dy), ..])` | press, move through each offset in turn, release at the last |
| `drag_to(t, onto)` | HTML drag and drop: `dragstart` … `drop`, `dragend`, one data transfer |
| `wheel(t, dx, dy)` | turns the wheel over it; what scrolls under it scrolls |
| `scroll(t, top, left)` | scrolls it to a position (`scroll` follows) |
| `resize(w, h)` | resizes the window; each `ResizeObserver` hears it |
| `focus(t)` | focus as Tab gives it: `:focus-visible` rings, nothing is clicked |
| `blur(t)` | takes focus away (`change` first when it was edited) |
| `type_text(t, s)` | focuses `t` and types `s` at the end, a character at a time |
| `set_value(t, s)` | focuses, selects all, types `s`: replaces the value |
| `paste(t, s)` | focuses, selects all, pastes `s` in one `paste` event, then one `input` (unless the app took it) |
| `clear(t)` | empties the field (select all, delete) |
| `select(t, value)` | picks the `<select>` option with that value: `input`, then `change` |
| `check(t)` / `uncheck(t)` | clicks a checkbox, radio or switch only when it isn't / is ticked |
| `run(\|\| ..)` | runs code in the app's root scope (an `act!`, a global write), then settles |
| `eval(js)` | runs JavaScript in the page, returns its value, then settles |

What a person can't do, a test can't either. Each of these fails,
saying why:

- clicking, typing into or dragging a hidden element, or one with no box;
- changing a `disabled` control (`can't click data-t="save": it is
  disabled`), or one inside a disabled `<fieldset>`;
- `select` of an option that isn't there or is disabled (it names the
  options there), `check` on something with no checked state, `focus` on
  something that can't take focus;
- `scroll` past the end, or of a box that doesn't scroll (it says where it
  stopped);
- a click whose element moved between press and release (a late layout
  shift): it went elsewhere, and the failure says where;
- a submit the browser refuses (a `required` field left empty): `the form
  can't be submitted: …`.

`type_text` and `set_value` type text, never keys: `\n` is no Enter, `\t`
no Tab (a line input drops a typed line break, as a browser drops a
pasted one). Press keys with `press`.

### Keys

| Call | Presses |
|---|---|
| `press(t, "Enter")` | a key, by its DOM name, on `t` (focused first, no click) |
| `press(t, "ctrl+s")` | a chord, as `use_global_key` writes it (`mod+k`: ⌘ on macOS, else Ctrl) |
| `press("window", "Escape")` | a key with nothing focused: what a global shortcut hears |
| `press_with(t, "Enter", Modifiers::CONTROL)` | a key holding modifiers (a modified Enter submits nothing) |
| `press_key(Key::ArrowDown, mods)` | a key on whatever has focus |
| `key_down(t, key, mods)` / `key_up(..)` | hold a key, then release it |
| `burst(&["alt+e", "a", "c"])` | chords as fast as a person types ahead: no step waits for the app |
| `TestUi::chord("mod+shift+k")` | the key and modifiers a chord means (pure) |

The browser does the rest: Enter in a form's line input clicks the
form's first submit button (with none, submits the form), Space on a
button clicks it, Tab moves focus, an arrow key moves between radios.
`ax trigger <app> window press ctrl+n` sends the same chord to a running
app.

A component alone, with its props, driven by keys and its form:

```rust,no_run
use aio::testing::{TestUi, fails};
use dioxus::prelude::*;

#[component]
fn SignIn(remember: bool) -> Element {
    let mut sent = use_signal(|| false);
    let mut name = use_signal(String::new);
    rsx! {
        form { onsubmit: move |e| { e.prevent_default(); sent.set(true) },
            label { "Name" input { required: true, value: "{name}", oninput: move |e| name.set(e.value()) } }
            label { "Remember me" input { r#type: "checkbox", checked: remember } }
            button { r#type: "submit", "Sign in" }
        }
        if sent() { p { "data-t": "hello", "Hello {name}" } }
    }
}

# fn main() {
let mut ui = TestUi::with(SignIn, SignInProps { remember: true });
assert!(ui.checked("RememberMeCheckbox"), "the prop reached the view");
assert!(ui.required("NameInput"));

// The browser refuses the empty required field, as it would a person.
let why = fails(|| ui.press("NameInput", "Enter"));
assert!(why.starts_with("the form can't be submitted"), "{why}");

ui.type_text("NameInput", "Ada");
ui.focus("NameInput");
assert_eq!(ui.info("NameInput").role, "textbox");
ui.press("NameInput", "Enter"); // submits, as the form's first submit button
assert_eq!(ui.text("hello"), "Hello Ada");
# }
```

## Reading

| Call | Answers |
|---|---|
| `text(t)` | its text, all descendants joined |
| `value(t)` | a field's value; a `<select>`'s chosen option's |
| `attr(t, "href")` | an attribute (`None`: not set) |
| `style(t, "color")` | a computed CSS property, custom ones too (`--gap`) |
| `rect(t)` | its box in the window, CSS px (`x`, `y`, `width`, `height`) |
| `checked(t)` | a checkbox, radio, or `role` with `aria-checked`, as last left |
| `disabled(t)`, `readonly(t)`, `required(t)` | its own attribute, or a disabled fieldset around it |
| `pressed(t)`, `expanded(t)` | `aria-pressed`, `aria-expanded` (always a `bool`) |
| `focused()` | the `data-t` of the element with focus (`None`: none, or no handle) |
| `info(t)` | its tag, role and accessible name |
| `present(t)`, `absent(t)`, `has_handle(t)`, `count(css)` | see [Showing means showing](#showing-means-showing) |
| `surface()` | what a person can see and use, nested, one line each |
| `html()` | the page's HTML without Dioxus's ids, then what the app put on `<body>` (a tooltip, a portal) |
| `contrast()` | text under WCAG AA on its background (4.5:1, 3:1 large), as `<tag> "text" 2.10:1, needs 4.5:1` |
| `read(&GLOBAL)` | a `GlobalSignal`'s value in the app's root scope |

A `surface()` line is `role "name" [handle] state`, indented by depth:

```text
  heading "Settings"
  textbox "Email" [email] = "ada@example.org"
  checkbox "Remember me" checked
  button "Save" [save] disabled
```

The same lines are what `ax surface <app>` prints for a running window,
and what a failure prints as the screen.

## Waiting

The views run when the test acts or waits, never behind its back.

| Call | Waits |
|---|---|
| `settle()` | until the views, the page and its events have nothing left to do; a commit made off the UI thread (an effect's) is shown first |
| `until_text(t, s)` | until `t` is there and shows exactly `s` (5 s) |
| `until_contains(t, part)` | until `t`'s text contains `part` (5 s) |
| `until(what, \|\| ..)` | until a condition, checked in the app's root scope, holds (5 s) |
| `until_within(d, what, \|\| ..)` | the same with your own patience |
| `expect_cell(&door, what, \|s\| ..)` | until a cell's state holds; the failure prints it (5 s) |
| `wait(d)` | lets `d` of real time pass for the app's timers (an interval, a toast's), then settles |

- Each wait fails with what it waited for, the screen as `surface()`
  shows it, and a trace file.
- `settle()` never gives up silently: a view and a page that keep
  answering each other fail (`the views and their page never settle`).
- Time in a `TestUi` is real: there is no virtual clock under the page.
  Test schedules and timers of the cell with
  [`TestCells::advance`](cells.md#time-and-schedules-advance), and keep
  the view's own timers short.

## Seeded state and the cell beside the page

An app made with `aio::client!(Cell, name)` has a `HANDLE` (its
`aio::Source`), the `name()` read and the `act!` calls. A test reaches
the same source:

```rust,no_run
use aio::act;
use aio::testing::{Mount, TestUi, seeded_source};
use dioxus::prelude::*;
use serde_json::json;

#[aio::cell(id = "inbox")]
pub struct Inbox {
    pub unread: u32,
    pub muted: bool,
}

#[aio::methods]
impl Inbox {
    pub fn read_all(&mut self) {
        self.unread = 0;
    }
}

aio::client!(Inbox, inbox);

static SHOW_ALL: GlobalSignal<bool> = Signal::global(|| false);

#[component]
fn App() -> Element {
    use_live();
    let n = *part(|s| &s.unread);
    rsx! {
        p { "data-t": "badge", "{n} unread" }
        button { disabled: n == 0, onclick: move |_| _ = act!(read_all()), "Mark all read" }
        button { onclick: move |_| *SHOW_ALL.write() = true, "Show all" }
    }
}

# fn main() {
// Start at the state under test: seeded before the first render.
let mut ui = Mount::default().before(|| *HANDLE.write() = seeded_source(json!({"unread": 3}))).app(App);
assert_eq!(ui.text("badge"), "3 unread");
ui.click("MarkAllReadButton");
ui.expect_cell(&*HANDLE.peek(), "all read", |s| s.unread == 0);
assert!(ui.disabled("MarkAllReadButton"));

ui.click("ShowAllButton");
assert!(ui.read(&SHOW_ALL), "a global, read in the app's root scope");
ui.run(|| *SHOW_ALL.write() = false);

// A call from the test, as a view would make it.
ui.run(|| *HANDLE.write() = seeded_source(json!({"unread": 1})));
assert_eq!(ui.text("badge"), "1 unread");
# }
```

- `seeded_source::<C>(json)` merges the seed into the default state, as
  [`TestCells::seeded`](cells.md#seeded-state) does: a key the state lacks
  fails.
- `expect_cell` takes any door: the source (`&*HANDLE.peek()`), a
  `Handle`, a `Remote`.
- `ui.run(f)` runs `f` where the app's globals live, then settles.

## Refusals nobody looked at

A view's call (`act!`, `call!`) that is refused gives the view `None` and
lands on the test's list. One nobody took fails the test at its end:
`refused and never looked at (take them with `failures`): add: say what
to do`. Take the expected ones with `ui.failures()` (`method: why`) and
assert on them. The same list holds the refusals of a served cell's
server (`ui_source` below) and the page's refused network requests.

## The network

By default a UI test refuses the real network: the page's requests to
any host but this machine are cancelled, and each fails the test unless
taken (`network: the page asked for https://…: a UI test refuses the real
network (allow its host with Mount::network)`). aio-rs's own outbound
requests in the test binary are held to the same rule. Requests to
`127.0.0.1` (a `TestServer`, a local stub) go through.

Let a host in with `Mount::default().network(&["api.example.org"])`, or
better, put a [cassette](cassettes.md) in front of the call so the test
needs no network at all.

## A served cell under the page

`TestUi::new(App)` runs the cell in-process. To test the view against the
real wire, put a copy from a `TestServer` in `HANDLE`:

```rust,no_run
use aio::act;
use aio::prelude::*;
use aio::testing::{Mount, TestServer};
use dioxus::prelude::*;
use std::time::Duration;

#[cell(shared, id = "votes")]
pub struct Votes {
    pub yes: u32,
}

#[methods]
impl Votes {
    #[client]
    pub fn vote(&mut self) {
        self.yes += 1;
    }

    pub fn reset(&mut self) {
        self.yes = 0;
    }
}

aio::client!(Votes, votes);

#[component]
fn App() -> Element {
    use_live();
    let yes = *part(|v| &v.yes);
    rsx! {
        p { "data-t": "yes", "{yes}" }
        button { onclick: move |_| _ = act!(vote()), "Vote" }
        button { onclick: move |_| _ = act!(reset()), "Reset" }
    }
}

# fn main() {
let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
let server = rt.block_on(TestServer::start(Handle::<Votes>::memory(), Default::default())).unwrap();
let source = rt.block_on(server.ui_source("ana", Duration::from_millis(30))).unwrap();
let mut ui = Mount::default().before(move || *HANDLE.write() = source).app(App);

ui.click("VoteButton");
assert_eq!(ui.text("yes"), "1", "predicted: shown at once");
ui.until("the server has it", || server.state().yes == 1); // a round trip later

ui.click("ResetButton"); // not a #[client] method: the server refuses it
ui.until("the refusal came back", || HANDLE.peek().pending(None) == 0);
let refused = ui.failures();
assert!(refused[0].starts_with("reset: "), "{refused:?}");
# }
```

- `ui_source(user, latency)` connects as `user` and holds every message
  `latency` each way (zero: as fast as the socket). Calls cross the wire
  encoding, and the server decides who may call what (`#[client]`, roles,
  rules): the test sees what a signed-in person's window sees.
- The server's refusals are the test's: take them with `failures()`.
- For sign-in itself (forms, cookies), use
  [`TestServer::page`](multi-app.md#sign-in-cookies-and-routes).

## Measuring

| Call | Answers |
|---|---|
| `components()` | the mounted components as a tree (`MountedComponent { name, renders, children }`): how often each rendered |
| `wire_bytes()` | bytes the views sent the page so far: a step's cost on the wire |
| `shot()` | a PNG of the page once its CSS transitions and animations ended (5 s at most) |
| `contrast()` | the text that fails WCAG AA, see [Reading](#reading) |

```rust,no_run
use aio::testing::{Mount, TestUi};
use dioxus::prelude::*;

#[component]
fn Row(n: u32) -> Element {
    rsx! { li { "row {n}" } }
}

#[component]
fn List() -> Element {
    let mut sel = use_signal(|| 0u32);
    rsx! {
        p { "data-t": "sel", "{sel}" }
        button { onclick: move |_| sel += 1, "Next" }
        ul { for n in 0..3u32 { Row { key: "{n}", n } } }
    }
}

# fn main() {
let mut ui = TestUi::new(List);
let before = ui.wire_bytes();
ui.click("NextButton");
let rows = &ui.components()[0].children;
assert!(rows.iter().all(|r| r.name == "Row" && r.renders == 1), "the rows didn't render again");
assert!(ui.wire_bytes() - before < 2_000, "one text changed, a small edit");

let shot = Mount::default().size(390, 844).dark().no_scrollbars().app(List).shot();
assert!(shot.starts_with(b"\x89PNG"), "a PNG to compare with a saved one");
# }
```

- A `shot()` is the window's engine drawing the live page, not a
  re-render. The mouse is moved off the page first unless `hover` put it
  somewhere, so a click leaves no stray `:hover` look.
- Compare shots across runs with `.no_scrollbars()` and a fixed `.size`:
  scrollbars differ by OS. `ax shot <app> --check base.png` does the same
  check on a running app.
- A render loop (a view that writes state it reads, rendering again and
  again with nothing from the page or the test in between) fails the
  test: `still rendering after … passes: a view writes state it reads`.

## When a test fails

A failed lookup, wait or `expect_cell` prints why, the screen as
`surface()` lines, and the path of a trace file:

```text
timed out after 5s waiting for count to show "2": it shows "1"
  screen:
    button "Add"
    heading "1" [count]
  trace: /tmp/aio-traces/notes-1a2b3c/tests--adds_twice/ui-1789112956688.json
```

The trace (JSON) holds why it failed, the handles, the surface, the HTML
and the page's warnings and errors. It goes under the temp folder at
`aio-traces/<test binary>/<test>`, keeps the 20 newest, and is removed
when the test passes. A failure taken on purpose with
[`fails`](cells.md#testing-a-failure-on-purpose-fails) writes none. If it
can't be written, the failure is unchanged and names no file.

## A video of a test

```sh
ax test notes --video videos/            # a video per UI test, named after it
ax test notes adds --video demo.webm     # the tests matching "adds", one file
ax test notes --video videos/ --pace 400 # each step 400 ms on screen
```

The test doesn't change. After each step that changed the page, the
page is captured by the browser itself; when the test ends, the frames
are encoded (`.mp4` H.264 or `.webm` VP8, by the extension; a folder
gets `<module>-<test>.mp4`). The default pace is 800 ms. `ax test` sets
`AIO_TEST_VIDEO` and `AIO_TEST_VIDEO_PACE`; `cargo test` with those
variables does the same.

## Canvas and 3D

A canvas has no elements to find: its drawing is pixels. Keep what it
shows in a cell and its decisions in plain functions, and test those:

- **Decisions are pure functions.** "What did the pointer hit", "which
  tile is visible", "is this drag a pan or a rotate" are functions of the
  camera, the pointer and the world. Write them without a canvas and test
  them as ordinary Rust.
- **State is in a cell.** The selection, the camera pose, the tool: test
  them with [`TestCells`](cells.md), then assert that the view's controls
  change them (`ui.drag_by("css=canvas", 40.0, 0.0)`, then `expect_cell`).
- **The drawing is checked by its pixels.** WebGL runs in the test page
  (drawn on the CPU), so a canvas app starts and draws in a `TestUi`.
  Check it with `shot()` against a saved image, or read a pixel with
  `ui.eval("…getImageData(…)…")`; on a running app, `ax shot <app> --check
  base.png` and `ax eval <app> <code>`.
- **Don't fake the context.** A stub `getContext` proves the stub; a path
  that needs the real GPU belongs in the thin drawing layer a shot checks.

## The running app, from outside

The page reader `TestUi` uses is the one a running window has, so a
test's names work on the app:

| Command | Does |
|---|---|
| `ax surface <app> [path] [--names] [--rects]` | the window's surface, its auto names, its boxes |
| `ax surface <app> --headless` | the app's views mounted headless on its running cells (a service with no window) |
| `ax trigger <app> <path> <action> [text]` | `click`, `type`, `setValue`, `press`, `select`, `check`, `scroll`, `dragTo`… as a person does |
| `ax shot <app> [file] [--check base]` | a PNG of the window; `--check` exits 1 unless it looks like `base` |
| `ax eval <app> <code>` | JavaScript in the window, its value as JSON |
| `ax preview <app> <component> [props]` | one component alone with props, its surface (or `--html`) |
| `ax components <app> [--renders]` | the mounted components and their renders |

`--wait-client ms` waits for the window right after a start or a reload
([Debugging](../debugging/README.md)).

## The web build: `TestBrowser`

`aio::testing::TestBrowser` launches a headless Chromium a test owns, for
the web build (`ax build --web`): `TestBrowser::open(url)` finds one
(`$CHROMIUM_BIN`, `$CHROME_BIN`, else the usual installs of Chromium,
Chrome, Edge and Brave), opens it on a profile of its own with a
DevTools port (`port()`), and kills it and removes the profile when
dropped. On Linux it reaches no display and no session bus. It only owns
the process: drive its page over the DevTools protocol.

## What aio has that aio-rs does without

| aio | aio-rs |
|---|---|
| happy-dom, no layout | a real engine: `rect`, `style`, `shot` and scrolling measure for real |
| `ui.App.SaveButton.click()` (a proxy) | `ui.click("SaveButton")`: names are strings, checked when the test runs |
| `am testgen` typed clients | `ax testgen <app>`: a Rust name table, a renamed element a compile error ([ax](../clients/ax.md#ax-testgen)) |
| `ui.advance(ms)`: a virtual clock under the page | none: `TestUi` time is real; schedules are tested with `TestCells::advance` |
| `testUI(App, { user })` | `TestServer::ui_source(user, latency)`: a real signed-in copy |
| `testPageFetch` (the page's HTTP and cookies) | `TestServer::page()`: the HTTP side, cookies kept |
| `wire: { strict: true }`, the JSON-loss table | none needed: arguments are Rust types the wire encodes; `ui_source` crosses the real wire |
| `ui.find("TodoRow", key)` by list key | a `data-t` per row (`row-{id}`), `list/MilkItem`, or `css=li:nth-child(2)` |
| `serverImport` stubs | a trait the effect calls, with a test implementation; or a [cassette](cassettes.md) |
| `smoke()` boot smoke | `ax build --smoke` |
| `uiRects` from a measured client | `rect(t)` in the test; `ax surface <app> --rects` |
| `--video-scheme` | `Mount::dark()` |

## Common mistakes

- Sleeping before a read: every action settles, and `until_text` waits
  for a change made off the UI thread. A `std::thread::sleep` only makes
  the test slower and still racy.
- `ui.text("Save")` for a button: its auto name is `SaveButton`. Run
  `ui.names()` (or read the failure's list) once.
- Asserting only the page: check the cell too (`expect_cell`), or a view
  that shows the right thing for the wrong reason passes.
- A `data-t` on every element: auto names are what a person reads, and
  they break when the label a person reads breaks. Use a handle for text
  without a role and for copy that will change.
- `type_text(t, "x\n")` to submit: `\n` is text. `press(t, "Enter")`.
- Leaving refusals untaken: the test fails at its end. Take them with
  `failures()` and assert on them.
- Testing a schedule's effect through the page by waiting minutes: test
  it with `TestCells::advance`, the view with a seeded state.
- Sharing state between tests through a `static`: each mount resets the
  app's globals, but not a `static` of your own.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`:
`aio::testing::TestUi`, `Mount`, `MountedComponent`, `Info`, `Key`,
`Modifiers`, `seeded_source`, `TestServer::ui_source`, `TestBrowser`,
`fails`; `aio::client!`, `aio::act!`, `aio::call!`, `aio::Source`.

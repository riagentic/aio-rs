# The UI kit

`aio::kit` (feature `kit`; aio's `aio/ui`) has the controls every app
would otherwise write again, as `aio::ui` components over real elements.
Each does its WAI-ARIA keyboard pattern, takes `class` (added to its own)
and any other attribute (`"data-t"`, `id`, `aria-*`, which win over the
kit's own), and is styled by the kit's stylesheet through `--aio-*`
tokens: put the theme on the page first
([theming](theming.md#the-pages-css)), or the controls show as bare HTML.

- **Use the kit** for forms, dialogs, menus, tabs, long lists, keys and
  toasts: the keyboard, focus and ARIA work is done and tested.
- **Write your own** element when nothing here fits; style it with the
  same tokens so it matches.

| Group | Components and hooks |
|---|---|
| Form | `Button`, `Field`, `Input`, `Textarea`, `Select`, `Checkbox`, `RadioGroup`, `Switch`, `use_form`, `use_async_rule`, `move_item` |
| Overlay | `Modal`, `Confirm`, `ConfirmButton`, `ToastHost` + `toast(..)`, `Tooltip`, `Menu`, `Portal` / `PortalHost` |
| Layout | `Card`, `Stack`, `Row`, `Tabs`, `Table`, `Pagination`, `Breadcrumb`, `Direction` |
| Status | `Spinner`, `Progress`, `Skeleton`, `Alert`, `EmptyState`, `ConnectionStatus`, `Avatar` |
| Lists | `VirtualList`, `window(..)`, `Transition`, `TransitionGroup` |
| Keys | `GlobalKeys`, `use_global_key`, `use_key_claim`, `Chord` |
| Other | `Markdown`, `ZoomPan`, `Browser` (a guest web page), `Island` / `WebIsland` (plain JS inside a view), `SignIn`, `ConnectPage`, `TimeTravelPanel`, `Defer`, `use_held`, `use_spring`, `use_frame`, `use_interval`, `use_window_event`, `use_window_listener`, `use_back_button` |

The parts a person uses inside a control carry handles of their own for
tests and agents: a tab's or menu item's id, `menuTrigger`, `menuScrim`,
`modalBackdrop`, `confirmOk`, `confirmCancel`, `toastDismiss`,
`alertDismiss`, `pagePrev`, `pageNext`, `page{n}`, `zoomIn`, `zoomOut`,
`zoomFit`, `zoomStage`, and the sign-in form's `signin…` parts.

## Setup

The kit's stylesheet comes with the theme: the app template passes
`aio::kit::theme_css(hue)` first to `aio::use_css!`, or
`aio::kit::theme::sheet(&cfg.ui, &cfg.app.id, ships_css)` to let
`[ui]` decide ([theming](theming.md#the-pages-css)). `aio::kit::CSS` is the
kit's rules alone, without tokens. Forget it and the controls render as
bare HTML (a `Menu` draws its items inline); a debug build says so once
in the log, naming the control. A rule of the app's own that names a kit
class (`.aio-btn`, `.aio-menu`) counts as styling it.

The kit's colours are `--aio-ui-*` tokens, each defaulting to the theme's
(`--aio-ui-accent` to `--aio-accent`…): set the theme's token to change
the app, the kit's to change the controls only
([kit tokens](theming.md#the-kits-own-tokens)).

## Buttons and fields

```rust
use aio::kit::{Button, Checkbox, Field, Input, Opt, RadioGroup, Select, Size, Switch, Textarea, Variant};
use aio::ui::prelude::*;

#[component]
fn Profile() -> View {
    let name = signal(String::new);
    let bio = signal(String::new);
    let lang = signal(|| "en".to_string());
    let size = signal(|| "M".to_string());
    let news = signal(|| false);
    let sync = signal(|| true);
    let mut saved = signal(|| 0);
    let missing = memo(move || name.with(|n| n.trim().is_empty()));
    rsx! {
        Field { label: "Name", hint: "As others see it", required: true,
            error: missing.get().then(|| "Your name, please".to_string()),
            Input { "data-t": "name", value: name.get(), placeholder: "Ada", oninput: move |v| name.set(v) }
        }
        Field { label: "About you",
            Textarea { "data-t": "bio", value: bio.get(), rows: 4, oninput: move |v| bio.set(v) }
        }
        Field { label: "Language",
            Select { "data-t": "lang", value: lang.get(), onchange: move |v| lang.set(v),
                options: vec![
                    Opt { label: "English".into(), ..Opt::from("en") },
                    Opt { label: "Deutsch".into(), ..Opt::from("de") },
                ],
            }
        }
        RadioGroup { label: "Size", options: vec![Opt::from("S"), Opt::from("M"), Opt::from("L")],
            value: size.get(), onchange: move |v| size.set(v) }
        Checkbox { "data-t": "news", label: "Send me news", checked: news.get(), onchange: move |on| news.set(on) }
        Switch { "data-t": "sync", label: "Sync", checked: sync.get(), onchange: move |on| sync.set(on) }
        Button { "data-t": "save", variant: Variant::Primary, size: Size::Lg, disabled: missing.get(),
            onclick: move |_| saved += 1, "Save" }
        p { "data-t": "out", "{saved} {lang} {size} {news} {sync}" }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Profile);
assert!(ui.disabled("save"), "nothing typed yet");
assert_eq!(ui.attr("name", "aria-invalid").as_deref(), Some("true"), "the Field's error marks its control");
ui.type_text("name", "Ada");
ui.select("lang", "de");
ui.check("news");
ui.click("sync");
ui.click("save");
assert_eq!(ui.text("out"), "1 de M true false");
# }
```

| Component | Props (besides `class` and any attribute) |
|---|---|
| `Button` | `variant: Variant` (`Primary`, `Secondary` default, `Ghost`, `Danger`), `size: Size` (`Sm`, `Md` default, `Lg`), `r#type` (`"button"` by default, so it never submits a form by accident; `"submit"`), `disabled`, `onclick`, `onmounted`, children |
| `Field` | `label`, `hint` (text, state as `"{n} left"`, or any view), `error` (shown, and the control marked `aria-invalid`), `required`, children: the control, which it labels and describes |
| `Input` | `value`, `r#type` (`"text"`), `placeholder`, `disabled`, `invalid`, `oninput(String)`, `onchange(String)`, `onkeydown` |
| `Textarea` | `value`, `placeholder`, `rows` (3), `disabled`, `invalid`, `oninput(String)`, `onkeydown` |
| `Select` | `options: Vec<Opt>`, `value`, `disabled`, `invalid`, `onchange(String)` |
| `Checkbox` | `checked`, `label`, `disabled`, `onchange(bool)` |
| `Switch` | `checked`, `label`, `disabled`, `onchange(bool)`: a real checkbox with `role="switch"` (Space toggles it) |
| `RadioGroup` | `options: Vec<Opt>`, `value`, `label` (or the `Field` around it), `name`, `disabled`, `onchange(String)`: native radios, arrows move |

- `Opt { value, label, disabled, view }`; `Opt::from("S")` is one whose
  label is its value; `.view(rsx! { .. })` shows any view beside its
  radio instead of the label, which still names it.
- Every control is controlled: it shows `value` / `checked` and tells its
  handler; keep the value in a signal or a cell and set it there.
- `aio::ui::btn("primary")` and `aio::ui::input_cls(extra, invalid)` give
  the same classes to a plain element.
- Forms with rules, touched fields and async checks: [forms](forms.md).

## Tabs, menus and navigation

```rust
use aio::kit::{Breadcrumb, Crumb, Menu, MenuItem, Pagination, Tab, Tabs};
use aio::ui::prelude::*;

#[component]
fn Settings() -> View {
    let tab = signal(|| "general".to_string());
    let picked = signal(String::new);
    let page = signal(|| 1u32);
    rsx! {
        Breadcrumb { items: vec![Crumb::new("Home").href("/"), Crumb::new("Settings")] }
        Tabs { label: "Settings", value: tab.get(), onchange: move |id| tab.set(id),
            tabs: vec![
                Tab::new("general", "General", rsx! { p { "data-t": "panel", "general" } }),
                Tab::new("about", "About", rsx! { p { "data-t": "panel", "about" } }),
                Tab::new("beta", "Beta", rsx! {}).disabled(),
            ],
        }
        Menu { label: "More", trigger: rsx! { "⋯" },
            items: vec![
                MenuItem::new("export", "Export"),
                MenuItem::new("reset", "Reset").disabled(),
                MenuItem::new("delete", "Delete").danger(),
            ],
            onselect: move |id: String| picked.set(id),
        }
        Pagination { page: page.get(), pages: 12, onpage: move |p| page.set(p) }
        p { "data-t": "state", "{picked} {page}" }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Settings);
ui.click("about");
assert_eq!(ui.text("panel"), "about");
ui.press("about", "ArrowRight"); // skips the disabled tab, wraps to the first
assert_eq!(ui.text("panel"), "general");
ui.click("menuTrigger");
ui.click("export");
ui.click("pageNext");
ui.click("page4");
assert_eq!(ui.text("state"), "export 4");
# }
```

| Component | Props |
|---|---|
| `Tabs` | `tabs: Vec<Tab>`, `value` (unset: it keeps its own, from the first enabled tab), `label`, `rtl`, `onchange(id)`. Arrows move (wrapping, skipping disabled tabs, mirrored right to left), Home and End jump; only the active tab is in the tab order, so Tab goes on into its panel. The panel renders only while active. |
| `Tab::new(id, label, panel)` | `.disabled()`; its id is its handle |
| `Menu` | `trigger: View`, `items: Vec<MenuItem>`, `label` (the trigger's name when it isn't text), `onselect(id)`. Enter, Space or ↓ open it on the first item (↑ on the last), arrows and Home/End move, Escape closes it and puts focus back on the trigger, Tab or a click outside closes it. Handles: `menuTrigger`, `menuScrim`, each item's id. |
| `MenuItem::new(id, label)` | `.disabled()`, `.danger()` |
| `Breadcrumb` | `items: Vec<Crumb>`, `separator` (text or any view; `/` unset), `label` (`"Breadcrumb"`); the last crumb is the page you are on (`aria-current="page"`), never a link |
| `Crumb::new(label)` | `.href(url)` or `.onclick(handler)`; neither: plain text |
| `Pagination` | `page` (from 1), `pages`, `onpage(u32)`, `window` (5 numbered pages), `label`. Targets are clamped, the button for the page shown is disabled. Handles `pagePrev`, `pageNext`, `page{n}`. |

## Layout and data

```rust
use aio::kit::{Align, Avatar, Card, Column, Row, Stack, Table};
use aio::ui::prelude::*;

#[derive(Clone, PartialEq)]
struct Item {
    id: u32,
    name: String,
    qty: u32,
}

#[component]
fn Stock() -> View {
    let opened = signal(String::new);
    let rows = vec![Item { id: 1, name: "Milk".into(), qty: 2 }, Item { id: 2, name: "Eggs".into(), qty: 12 }];
    rsx! {
        Card { title: rsx! { "Stock" }, footer: rsx! { "2 items" },
            Stack { gap: "8px",
                Row { Avatar { name: "Ada Lovelace" } span { "Ada's shop" } }
                Table {
                    columns: vec![
                        Column::text("Item", |r: &Item| r.name.clone()),
                        Column::text("Qty", |r: &Item| r.qty.to_string()).align(Align::End).width("6rem"),
                    ],
                    rows,
                    key_of: |r: Item| r.id.to_string(),
                    onrow: move |(r, _i): (Item, usize)| opened.set(r.name),
                }
            }
        }
        p { "data-t": "opened", "{opened}" }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Stock);
ui.click("row-2");
assert_eq!(ui.text("opened"), "Eggs");
ui.press("row-1", "Enter");
assert_eq!(ui.text("opened"), "Milk");
# }
```

| Component | Props |
|---|---|
| `Card` | `title: View`, `footer: View` (both optional), children |
| `Stack` | `gap` (`"12px"`, any CSS length), `align`, children: a column |
| `Row` | `gap`, `align`, children: a wrapping row |
| `Table<R>` | `columns: Vec<Column<R>>`, `rows: Vec<R>`, `key_of` (a stable key per row; the index by default), `empty` (shown with no rows: `"No rows"`), `onrow((R, index))`: then a row is in the tab order and Enter or Space opens it. Row handles `row-{key}`. |
| `Column::text(header, \|r\| ..)`, `Column::view(header, \|r, i\| rsx! { .. })` | `.align(Align::Start / Center / End)`, `.width("6rem")` |
| `Avatar` | `name`, `src` (an image; else initials on a colour from the name), `size` (32 px), `color` |
| `Direction` | `rtl`, children: a part of the page that runs right to left; the kit inside follows it |

## Status

| Component | Props |
|---|---|
| `Spinner` | `label` (`"Loading"`), announced to screen readers |
| `Progress` | `value: Option<f64>` (`None`, or not finite: the indeterminate bar), `max` (100), `label`, `show_value` (the percent beside it) |
| `Skeleton` | `width`, `height` (CSS lengths), `circle`, `lines` (1); hidden from screen readers, so put `aria-busy` on the region |
| `Alert` | `tone: Tone` (`Info` default, `Success`, `Warn`, `Error`), `title`, `ondismiss` (then a × button, `alertDismiss`), children; only `Error` interrupts (`role="alert"`) |
| `EmptyState` | `title`, `icon` (a glyph or any view), `description`, `action: View`: what an empty list shows, and what to do |
| `ConnectionStatus` | `status: aio::Status` (`use_status().get()`): "Reconnecting…" while the link is down, then "Connected" for two seconds; once, in the root |
| `Tooltip` | `text`, `placement` (`Placement::Top` default, `Bottom`, `Left`, `Right`), `bubble_id`, children: shown on hover and on keyboard focus, closed by Escape; a kit control inside is described by it, a plain element takes `bubble_id` (from `use_id`) as its `aria-describedby` |

## Dialogs and toasts

```rust
use aio::kit::{Confirm, ConfirmButton, Modal, ToastHost, Tone, Variant, toast, toasts};
use aio::ui::prelude::*;

#[component]
fn Lists() -> View {
    let ask = signal(|| false);
    let info = signal(|| false);
    let mut lists = signal(|| 3);
    rsx! {
        ToastHost {}
        button { "data-t": "delete", onclick: move |_| ask.set(true), "Delete one" }
        Confirm {
            open: ask.get(),
            title: "Delete the list?",
            message: "Its items go too.",
            confirm_label: "Delete",
            danger: true,
            onconfirm: move |_| { ask.set(false); lists -= 1; toast("Deleted", Tone::Success); },
            oncancel: move |_| ask.set(false),
        }
        ConfirmButton { "data-t": "wipe", variant: Variant::Danger, confirm: "Delete every list?",
            onconfirm: move |_| lists.set(0), "Delete all" }
        button { "data-t": "about", onclick: move |_| info.set(true), "About" }
        Modal { open: info.get(), title: "About", onclose: move |_| info.set(false),
            footer: rsx! { button { "data-t": "close", onclick: move |_| info.set(false), "Close" } },
            p { "Lists, version 1" }
        }
        p { "data-t": "count", "{lists}" }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Lists);
ui.click("delete");
ui.click("confirmOk");
assert_eq!(ui.text("count"), "2");
assert_eq!(ui.run(toasts), ["Deleted"]);
ui.click("wipe");
ui.click("confirmCancel");
assert_eq!(ui.text("count"), "2", "cancelled: nothing happened");
ui.click("about");
ui.press("close", "Escape");
assert!(ui.absent("close"), "Escape closed the dialog");
# }
```

| Component or call | Props or arguments |
|---|---|
| `Modal` | `open`, `onclose` (Escape and a backdrop click, `modalBackdrop`, call it when `dismissable`, the default), `title`, `footer: View`, children. Focus stays inside while it is open and goes back where it was; the latest opened is on top; it renders nothing while closed. |
| `Confirm` | `open`, `onconfirm`, `oncancel` (also Escape and the backdrop), `title`, `message`, `confirm_label` (`"Confirm"`), `cancel_label` (`"Cancel"`), `danger`. Handles `confirmOk`, `confirmCancel`. |
| `ConfirmButton` | `confirm` (the question), `onconfirm` (only once confirmed), `confirm_title`, `confirm_label`, `cancel_label`, `variant` (`Danger` makes the confirm button danger too), `disabled`, children; focus returns to it when the dialog closes |
| `ToastHost` | once, in the root: a polite live region (an error toast is an alert) |
| `toast(text, tone)` | shows for 4 s; gives a `Toast` with `.dismiss()` |
| `toast_for(text, tone, stay)` | `stay: Option<Duration>`; `None`: until dismissed (its ×, `toastDismiss`) |
| `toasts()` | the messages showing, oldest first (for tests and agents) |
| `Portal { children }` / `PortalHost` | draws its children in the host (put it around the app once) while it lives: a popover that must escape an `overflow: hidden` box ([islands](islands.md)); `Portal { target: "side" }` draws in the `PortalHost { name: "side" }` showing anywhere on the page (aio's `target`; none showing: in place) |

Call `toast` on the UI thread (a handler, a view's task). From a method
or an effect, tell people with `cx.tell(..)` instead
([effects](../state/effects.md#telling-people)).

## Keys

```rust
use aio::kit::{Chord, GlobalKeys, use_global_key, use_key_claim};
use aio::ui::prelude::*;

static COUNT: Global<i32> = Global::new(|| 0);

#[component]
fn App() -> View {
    rsx! { GlobalKeys { Editor {} } } // the app's root: bindings fire under it
}

#[component]
fn Editor() -> View {
    // Fires once while held: only movement keys repeat by default.
    use_global_key("mod+s", |_| *COUNT.write() += 1);
    use_global_key("ArrowDown", |_| *COUNT.write() += 1); // repeats
    use_global_key(Chord::parse("mod+z").unwrap().repeating(true), |_| *COUNT.write() -= 1);
    // Taken before any element hears it (aio's capture listener).
    use_key_claim("Tab", |_| {});
    rsx! {
        // The page keeps these keys from the browser inside this element.
        div { "data-t": "editor", tabindex: "0", "data-aio-prevent": "ArrowUp ArrowDown PageUp PageDown Home End Tab", "{COUNT}" }
    }
}
# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.press("editor", "ctrl+s");
assert_eq!(ui.text("editor"), "1");
# }
```

- A chord is modifiers then a key, joined by `+`: `ctrl`, `shift`, `alt`,
  `meta`, `mod` (Ctrl, or ⌘ on macOS), then a character or a DOM key name
  (`Enter`, `esc`, `space`, `ArrowUp`/`up`, `F1`). `TestUi::press` and
  `ax trigger <app> window press ctrl+n` take the same words.
- Bindings don't fire while typing in a field unless they say so; a key
  pressed with nothing focused reaches them too.
- `e.prevent_default()` in a handler comes too late in a window (the
  browser moved on before the view hears the key): put
  `data-aio-prevent` on the element instead.
- Only arrows, PageUp/PageDown and Backspace repeat when held (aio's
  `REPEATABLE_KEYS`); `Chord::repeating(bool)` overrides.

## Forms

```rust
use aio::kit::{Field, Input, Problems, use_form};
use aio::ui::prelude::*;

#[derive(Clone, Default, PartialEq)]
struct Signup {
    name: String,
    age: Option<u32>,
}

/// One function names the field each problem is about.
fn check(s: &Signup) -> Problems {
    let mut p = Problems::new();
    if s.name.trim().is_empty() {
        p.push(("name", "Your name, please".into()));
    }
    if s.age.is_some_and(|a| a < 18) {
        p.push(("age", "18 or older".into()));
    }
    p
}

#[component]
fn Form() -> View {
    let f = use_form(Signup::default, check);
    rsx! {
        form { onsubmit: move |e| { e.prevent_default(); if let Some(_ok) = f.submit() { /* act!(..) */ } },
            Field { label: "Name", error: f.error("name"),
                Input { "data-t": "name", value: f.values().name, oninput: move |n| f.edit("name", |s| s.name = n), onchange: move |_| f.touch("name") }
            }
            Field { label: "Age", error: f.error("age"),
                Input { r#type: "number", oninput: move |n: String| f.edit("age", |s| s.age = n.parse().ok()) }
            }
            button { "data-t": "submit", r#type: "submit", "Sign up" }
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Form);
assert_eq!(ui.attr("name", "aria-invalid"), None, "no error before a submit or a leave");
ui.click("submit");
assert_eq!(ui.attr("name", "aria-invalid").as_deref(), Some("true"));
# }
```

A field's error shows once it was left or a submit was tried.
`use_async_rule(f, "name", |s| s.name.clone(), debounce, |name| async ..)`
adds a rule that waits (a uniqueness lookup). Enter in a form with no
submit button submits it, as in aio; one whose button the view enables a
frame later is submitted once it is. `move_item(&mut rows, from, to)`
reorders a field array (aio's `useFieldArray`). Every part of it, with
validation, submitting and server errors: [forms](forms.md).

## Long lists

```rust
use aio::kit::VirtualList;
use aio::ui::prelude::*;

#[component]
fn Names(names: Vec<String>) -> View {
    rsx! {
        VirtualList {
            len: names.len(),
            row_height: 28.0,
            height: 400.0,
            row: move |i: usize| rsx! { div { class: "name", "{names[i]}" } },
        }
    }
}

#[component]
fn App() -> View {
    rsx! { Names { names: (0..10_000).map(|i| format!("person {i}")).collect::<Vec<_>>() } }
}

# fn main() {
let ui = aio::testing::TestUi::new(App);
let drawn = ui.count(".name");
assert!(drawn > 10 && drawn < 30, "{drawn} of 10000 rows are elements");
# }
```

| `VirtualList` prop | Is |
|---|---|
| `len` | how many rows |
| `row_height`, `height` | px; every row is `row_height` tall, the box `height` |
| `overscan` | rows drawn beyond each edge (3) |
| `scroll_to: Option<usize>` | a change scrolls that row to the top |
| `row: \|i\| rsx! { .. }` | the row at an index |

Only the rows the scroll shows, and `overscan` more each side, are
elements; a scroll within the rows drawn writes nothing; `scroll_to`
brings a row to the top. A keyboard list windowed by hand uses
`aio::kit::window(len, row, height, overscan, top)` and
`data-aio-reveal` on the cursor's row ([performance](../performance.md)).

## Motion

`Transition`, `TransitionGroup` (rows in, out, and sliding to where they
moved), `use_spring`, `use_frame` and `use_interval` are on
[animation](animation.md).

| Item | Is |
|---|---|
| `Transition { show, enter, exit, children }` | content that plays CSS keyframes as it comes and goes; the leaving content stays until its exit is over |
| `Motion::FADE`, `SLIDE`, `SCALE` (300 ms), `Motion::new("keyframes")`, `.ms(..)` | what plays |
| `TransitionGroup { items: Vec<(key, View)>, enter, exit, flip, flip_ms, row_class }` | a keyed list whose rows animate; `flip` slides moved rows |
| `use_spring(initial, Spring::default())` | a number springing to `.to(target)` (stiffness 170, damping 26); `.value()` |
| `use_frame(active, \|since_start, since_last\| ..)`, `use_interval(every, active, \|\| ..)` | per frame, on a timer, while `active` |

## Islands and other sites

```rust
use aio::kit::{Island, WebIsland};
use aio::ui::prelude::*;

#[derive(aio::serde::Serialize, Clone, PartialEq)]
#[serde(crate = "aio::serde")]
struct Chart {
    series: Vec<i32>,
}

/// The body of a function returning `mount(el, props, emit)`.
const CHART: Island = Island::js("return (el, p) => { el.textContent = p.series.join(','); };");

#[component]
fn Sales() -> View {
    rsx! {
        WebIsland::<Chart, ()> {
            island: CHART,
            props: Chart { series: vec![1, 2] },
            "data-t": "chart",
            loading: rsx! { span { "loading" } },
        }
    }
}
# fn main() {
let mut ui = aio::testing::TestUi::new(Sales);
ui.until_text("chart", "1,2");
# }
```

- An island is a box of the page drawn by the app's own JavaScript (a
  chart or map library, a React component with `Island::react`): props
  in, events out through `emit` to `onevent`. `mount` may return
  `{ update(props), unmount() }`; a prop change calls `update` in place.
- Its script runs in the page with the app's rights. A library the app
  doesn't vouch for goes in `Island::js(..).sandboxed()`: an iframe of its
  own origin, with no page DOM, wire, storage or cookies.
- The page loads nothing from the network: bundle the library into one
  script and give it with `include_str!`.
- `Browser { src }` shows someone else's site in the view (Electron's
  `<webview>`, sandboxed, its own session), when the window allows it
  ([views](views.md#the-window)); `keep_alive` remembers where it was,
  `host_keys` gives keys back to the app, `onevent` hears navigations.
- More on islands, foreign widgets, portals and other sites:
  [islands](islands.md).

## Markdown, zoom, deferred content

```rust
use aio::kit::{Defer, Markdown, Trigger, ZoomPan, ZoomView};
use aio::ui::prelude::*;

#[component]
fn Doc() -> View {
    let view = signal(ZoomView::default);
    rsx! {
        Markdown { "data-t": "md", source: "# Notes\n\n**Milk**, *eggs* and `bread`. [Shop](https://example.com/)" }
        ZoomPan { "data-t": "map", style: "width: 300px; height: 200px", onview: move |v| view.set(v),
            img { src: "map.png", alt: "Map" }
        }
        p { "data-t": "scale", {view.with(|v| v.scale.to_string())} }
        Defer { "data-t": "more", trigger: Trigger::Interaction, placeholder: rsx! { "Show the comments" },
            p { "data-t": "comments", "Comments" }
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Doc);
assert_eq!(ui.count("[data-t=md] strong"), 1);
assert_eq!(ui.attr("css=[data-t=md] a", "target").as_deref(), Some("_blank"), "a link out opens outside");
ui.click("zoomIn");
assert_ne!(ui.text("scale"), "1");
assert!(ui.absent("comments"));
ui.click("more");
assert!(ui.present("comments"));
# }
```

| Component | Props |
|---|---|
| `Markdown` | `source`: headings, paragraphs, **strong**, *em*, `code`, fenced code, quotes, lists, rules, links and images, as safe elements (no raw HTML; `javascript:` and `data:` links dropped; an `http(s)` link opens outside) |
| `ZoomPan` | `min_scale` (0.1), `max_scale` (10), `initial: ZoomView { x, y, scale }`, `onview(ZoomView)`, `controls` (the ＋ － fit buttons: `zoomIn`, `zoomOut`, `zoomFit`; true), children; size it with `class` or `style`. The wheel zooms at the pointer, a drag pans (`zoomStage`). `aio::kit::zoom_at` and `fit_view` are its pure math. |
| `Defer` | `trigger: Trigger` (`Now`, `After(d)`, `Idle`: after the first frame, `Viewport`: when it scrolls into view, `Hover`, `Interaction`: a click or key on it), `placeholder: View`, children |

## Other parts

| Item | Is |
|---|---|
| `TimeTravelPanel { timeline: use_time_travel().get(), onask: move \|a\| _ = time_travel(a) }` | the cell's history in a corner, hidden until Ctrl+.; undo, redo, pause, a step to go to (handles `timeTravel`, `ttUndo`, `ttRedo`, `ttPause`) |
| `ConnectPage { data, app }` | a client's connect page: an address and a pairing code, the app's servers on the LAN, the recent servers, why it shows (`use_connect_page`); the desktop and Android shells show it in a client's window by themselves ([desktop](../clients/desktop.md#the-connect-page)); parts `connectWhy`, `connectAddr`, `connectCode`, `connectGo`, `connectError`, `connectLook`, `near{n}`, `recent{n}`, `forget{n}` |
| `SignIn { client, on_signed }` (feature `server`) | a ready sign-in form: sign-up when the server allows it (`signup`), an email (`email`), the second-factor step, a forgotten password when the server sends mail (`forgot`), `title`; parts `signinId`, `signinEmail`, `signinPassword`, `signinCode`, `signinSubmit`… ([auth](../auth/README.md)) |
| `use_id(prefix)` | a stable, unique id per component, for `aria-controls`, `for` |
| `use_context_select(\|t: &T\| ..)` | a part of a `Signal<T>` context; follows only when it changes |
| `use_held(&pool, key)` | a held resource (a connection, a file) for this view, let go when the key changes or the view goes |
| `use_window_event(\|e\| ..)`, `WindowEvent` | the window's `MouseMove` (every `MOVE_MS`, 100 ms), `MouseDown`, `MouseUp`, `Click`, `Wheel`, `KeyDown`, `KeyUp`, `Resize { width, height }`; inside `GlobalKeys` |
| `use_window_listener("visibilitychange", \|e\| ..)` | any DOM event type the window hears (`online`, `scroll`, `pointermove` unthrottled…), its plain fields (`type`, `key`, `clientX`…) as JSON, each one; removed with the component (aio's `onWindowEvent(type)`) |
| `use_back_button(\|\| ..)` | Back while the view lives; true: taken ([routing](routing.md#back)) |
| `Browser { src, .. }` | someone else's site in the view ([islands](islands.md)) |

Labels and titles are markup, as aio's `string | VNode`: every label and
title is a `View` that takes text too: `Field`, `Checkbox`, `Switch`,
`Alert`, `Modal`, `Confirm`, `ConfirmButton`, `EmptyState`, `Tab::new`,
`MenuItem::new`, `Crumb::new`, `Column` headers, and `toast_view` for a
toast (`label: rsx! { b { "Name" } }`, or `label: "{n} picked"`, which
follows `n`).

## Gaps against aio

The kit matches aio's `aio/ui` component for component and adds the
controls aio apps wrote by hand (`Tabs`' right-to-left keys, `Defer`,
`ZoomPan`'s math, `TransitionGroup`); what aio has beyond it:

- **`<UiStyles/>`**: none; the stylesheet comes with the theme
  (`theme_css`, `theme::sheet`) rather than a component.
- **Data grids, charts, date pickers**: neither has them; mount a
  library as an [island](islands.md).

## Common mistakes

- `TestUi::press` on a plain `div`: keys go to what has focus, and a
  `div` can't take it; give it `tabindex: "0"` or press on a control.
- No theme on the page: the controls render as bare HTML (a dev build
  says so once).
- A controlled control without its handler: `Switch { checked }` with no
  `onchange` never changes; keep the value in a signal or a cell.
- Two `ToastHost`s or `GlobalKeys`: one each, in the root.
- `prevent_default()` on keys in a window: use `data-aio-prevent`.
- A `VirtualList` with rows of different heights: it assumes
  `row_height`; window by hand with `aio::kit::window` instead.
- `Button` inside a `form` expected to submit: its type is `"button"`;
  write `r#type: "submit"`.
- `toast(..)` from a cell method or effect: it belongs to the UI thread;
  use `cx.tell(..)`.
- A `Field` around two controls: it labels the first; give each its own.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`, module `aio::kit`:
every component's props, each hook, `theme`, `markdown`.

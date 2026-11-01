# Views

Views are [Dioxus](https://dioxuslabs.com) components (`rsx!`): real HTML
and CSS. The same views are drawn by the shared Electron runtime on the
desktop, by the system WebView on Android and by the browser for
`ax build --web` ([targets](../build/targets.md)). A cell holds the state;
a view reads it and calls its methods. Views need aio-rs's feature `kit`
(the app template has it).

- **Use a view** for everything a person sees. Keep state the app must keep
  (a selection worth restoring, a draft) in a [cell](../state/cells.md);
  view-only state (hover, an open menu) in Dioxus signals.
- **Not a view**: a command-line tool ([cli apps](../clients/cli-apps.md))
  or a headless service (`ax create --template service`).

## A cell's client

```rust
use aio::act;
use dioxus::prelude::*;

#[aio::cell(persist, coalesce, id = "tasks")]
pub struct Tasks {
    pub items: Vec<String>,
    pub draft: String,
}

#[aio::methods]
impl Tasks {
    pub fn type_draft(&mut self, text: String) {
        self.draft = text;
    }

    pub fn add(&mut self) -> aio::Result {
        aio::ensure!(!self.draft.trim().is_empty(), "type a task first");
        self.items.push(std::mem::take(&mut self.draft));
        Ok(())
    }
}

// SOURCE, HANDLE, part(..), tasks() (the whole state), use_status(),
// use_pending(method), use_time_travel(), use_live() and the door act! calls.
aio::client!(Tasks, tasks);

#[component]
pub fn App() -> Element {
    use_live(); // once, in the root: commits from effects re-render too
    rsx! {
        main { Draft {} List {} }
    }
}

#[component]
fn Draft() -> Element {
    let draft = part(|t| &t.draft); // re-renders only when `draft` changes
    rsx! {
        form { onsubmit: move |e| { e.prevent_default(); _ = act!(add()); },
            input { "data-t": "draft", value: "{draft}",
                oninput: move |e| _ = act!(type_draft(e.value())) }
            button { r#type: "submit", disabled: draft.trim().is_empty(), "Add" }
        }
    }
}

#[component]
fn List() -> Element {
    let items = part(|t| &t.items);
    rsx! {
        ul { "data-t": "list",
            for (i, item) in items.iter().enumerate() {
                Item { key: "{i}", text: item.clone() }
            }
        }
    }
}

/// A row by props: Dioxus skips it while its props are equal.
#[component]
fn Item(text: String) -> Element {
    rsx! { li { "{text}" } }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.type_text("draft", "milk");
ui.click("AddButton");
assert_eq!(ui.text("list"), "milk");
# }
```

In the app's launcher, `main` sets the client's source before the window
opens; until then (in tests) the cell is an in-memory one:

```rust,ignore
// ax create's template (app/src/main.rs), compiled by ax's own tests:
let cfg = aio::config!();
let Start::Run(dirs) = aio_shell::start(&cfg, aio::app_version!())? else { return Ok(()) };
_ = app::SOURCE.set(aio::Source::for_app(&cfg, &dirs)?);
aio_shell::run(app::App, aio_shell::Window { width: 900, height: 640, ..aio_shell::Window::of(&cfg) }, &dirs)
```

### How it fits together

```text
aio.toml ─ aio::config!() ─ main.rs: SOURCE.set(Source::for_app(..)) ─ aio_shell::run(App, ..)
                                              │
  a person clicks ─ handler ─ act!(add()) ─ the cell's method ─ commit ─ the views that read
                                                                   │     a changed field re-render
                                     effects, schedules, sync ─────┘     (at most every 150 ms)
```

- A **cell** owns the state and its rules; its methods are the only way
  to change it, and every change is an event in its log.
- A **view** reads the state (`part`, `name()`), shows it, and turns what
  a person does into calls (`act!`).
- The **window** (the shared Electron runtime, Android's WebView, a
  browser tab) shows the page the views make; nothing in a view depends
  on which one it is.
- The same views run headless in `TestUi` (a real page, no window), so
  every example on this page is a test.

### What `client!` makes

`aio::client!(Cell, name)` makes these, once per app instance (so once
per test):

| Item | What it is |
|---|---|
| `SOURCE` | where the cell lives (`aio::Source::for_app(&cfg, &dirs)`: this process's store, or a synced copy of a server's) |
| `part(\|c\| &c.field)` | one top-level field; the view re-renders only when it changes |
| `name()` | the whole state (`Arc<Cell>`); in a render it re-renders on every commit |
| `use_live()` | in the root, once: commits from effects, schedules and sync re-render too |
| `use_status()` | how the copy stands: `online`, `pending`, `degraded`, `conn`, `last_sync`, `rtt` |
| `use_pending(Some("method"))` | calls of that method in flight (`None`: all), for a spinner |
| `use_time_travel()`, `time_travel(ask)` | the cell's history and steps through it (`kit::TimeTravelPanel`) |
| `act!(method(args))` | calls a method from a handler |

`aio::client!(Cell, name, name_at)` also makes `name_at(|c| value)`: a hook
on a value made from the state, re-rendering when that value changes.

## Reading and calling

- `part(|c| &c.field)` subscribes the view to one top-level field: pick the
  field, then index it (`part(|c| &c.rows)[i]`, not `part(|c| &c.rows[i])`).
  `.ver()` is the field's stamp, for caches
  ([performance](../performance.md#1-read-fields)).
- `name()` reads the whole state. Use it in handlers, which don't
  subscribe; a dev build names, once, each render that reads it (`[hint]`
  in the log).
- `act!(method(args))` calls the method; a refusal is logged and gives
  `None`, an answer `Some(value)`. A view's call renders at once; commits
  from effects, schedules and other threads render at most once per
  `aio::ui::BACKGROUND` (150 ms).
- A refusal is the cell's last word, not the view's message: check the
  input in the view first (as `Draft` disables its button), so a person
  never meets a refusal they can't read.

## Components, props and children

A component is a function returning `Element`; `#[component]` turns its
arguments into props. A component whose props are equal (`PartialEq`)
doesn't render again, so rows that take their data by props stay put
when the list around them changes.

```rust
use dioxus::prelude::*;

#[derive(Clone, PartialEq)]
struct Contact {
    id: u32,
    name: String,
    fav: bool,
}

/// `children` is what the caller puts inside; `note` may be left out.
#[component]
fn Panel(title: String, #[props(default)] note: Option<String>, children: Element) -> Element {
    rsx! {
        section { class: "card",
            h2 { "{title}" }
            if let Some(n) = note {
                p { class: "muted", "data-t": "note", "{n}" }
            }
            {children}
        }
    }
}

#[component]
fn Contacts() -> Element {
    let mut people = use_signal(|| {
        vec![Contact { id: 1, name: "Ada".into(), fav: true }, Contact { id: 2, name: "Linus".into(), fav: false }]
    });
    let mut field = use_signal(|| None::<std::rc::Rc<MountedData>>);
    let none = people.read().is_empty();
    rsx! {
        Panel { title: "Contacts", note: "people you know",
            ul {
                for c in people.read().iter() {
                    // key: what keeps a row's element (and its state) with its data
                    li { key: "{c.id}", class: aio::ui::cx(&[("row", true), ("fav", c.fav)]), "{c.name}" }
                }
            }
            if none {
                p { "data-t": "empty", "No one yet" }
            }
            // A ref: the element, once mounted, for what only it can do.
            input { "data-t": "search", onmounted: move |e| field.set(Some(e.data())) }
            button { "data-t": "find",
                onclick: move |_| async move {
                    if let Some(el) = field() {
                        _ = el.set_focus(true).await;
                    }
                },
                "Find"
            }
            // `false` leaves a boolean attribute out.
            button { "data-t": "clear", disabled: none, onclick: move |_| people.write().clear(), "Clear" }
            svg { "data-t": "dot", width: "8", height: "8", circle { cx: "4", cy: "4", r: "4" } }
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Contacts);
assert_eq!(ui.count("li.fav"), 1);
assert_eq!(ui.text("note"), "people you know");
ui.click("find");
assert_eq!(ui.focused().as_deref(), Some("search"));
ui.click("clear");
assert!(ui.present("empty"));
assert!(ui.disabled("clear"));
assert_eq!(ui.count("li"), 0);
# }
```

| Prop form | Means |
|---|---|
| `name: String` | required |
| `#[props(default)] n: u32` | may be left out: `Default` |
| `#[props(default = 3)] rows: u32` | may be left out: that value |
| `#[props(into)] label: String` | takes `&str` too |
| `note: Option<String>` | may be left out (`None`); `note: "x"` gives `Some` |
| `children: Element` | what the caller writes inside |
| `onpick: EventHandler<String>` | a handler the caller passes; `onpick.call(v)` |
| `#[props(extends = GlobalAttributes)] attributes: Vec<Attribute>` | any HTML attribute, spread with `..attributes` |

### Markup

| Want | Write |
|---|---|
| text with values | `"{name} ({count})"` (format args; `{x:?}` too) |
| an attribute | `class: "row"`, `"data-t": "x"`, `"aria-label": "Close"` |
| a value that may be absent | `title: tip` where `tip: Option<String>`: `None` leaves it out |
| classes by condition | `class: aio::ui::cx(&[("row", true), ("on", on)])` |
| inline style | `style: "width: {w}px"`, or one property: `width: "{w}px"` |
| a condition | `if on { X {} } else { Y {} }`, `if let Some(x) = .. { .. }` |
| a match | `match page { A => rsx! { .. }, B => rsx! { .. } }` |
| a list | `for x in xs { li { key: "{x.id}", .. } }`, or `{xs.iter().map(\|x\| rsx! { .. })}` |
| several roots | a component may return them side by side: `rsx! { A {} B {} }` |
| an element made elsewhere | `{elem}` where `elem: Element` |
| trusted HTML | `div { dangerous_inner_html: "{html}" }`: never with text a person typed; the kit's `Markdown` is safe |
| SVG | `svg { path { d: "M0 0L8 8" } }`, attributes as SVG names them |

### Events

| Handler | Gets | Typical |
|---|---|---|
| `onclick`, `ondoubleclick`, `oncontextmenu` | `MouseEvent` | `move \|_\| _ = act!(save())` |
| `oninput`, `onchange` | `FormEvent`: `e.value()`, `e.checked()` | a field |
| `onsubmit` | `FormEvent` | `e.prevent_default()`, then the call |
| `onkeydown`, `onkeyup` | `KeyboardEvent`: `e.key()`, `e.modifiers()` | a field's Enter |
| `onfocus`, `onblur` | `FocusEvent` | |
| `onpointerdown`, `onpointermove`, `onwheel` | `PointerEvent`, `WheelEvent` | a drag |
| `onmounted` | `MountedEvent`: `e.data()` | focus, scroll, measure |
| `onscroll`, `onresize` | | |

- A handler may be `async` (`move |_| async move { .. }`): it runs as a
  task of the component, dropped with it.
- `e.stop_propagation()` keeps the event from the elements around;
  `e.prevent_default()` from the browser, but keys in a window need
  `data-aio-prevent` ([kit](kit.md#keys)).
- A key that should work anywhere in the window is a global key
  (`aio::kit::use_global_key`), not an `onkeydown` on a `div`.

## Local state and hooks

State only a view needs (an open menu, a hover, a half-typed search)
lives in Dioxus signals; state the app keeps lives in a cell. A hook is a
`use_…` call at the top of a component: hooks are matched by the order
they are called in, so never call one behind an `if`, in a loop, or after
an early `return`.

```rust
use dioxus::prelude::*;

/// One per app instance (per test), shared by every component.
static THEME: GlobalSignal<&'static str> = Signal::global(|| "light");

#[component]
fn Counter() -> Element {
    let mut n = use_signal(|| 1);
    let double = use_memo(move || n() * 2); // follows `n`
    let mut log = use_signal(Vec::<i32>::new);
    // Runs after a render that changed what it reads.
    use_effect(move || log.write().push(double()));
    rsx! {
        button { "data-t": "inc", onclick: move |_| n += 1, "{n}" }
        p { "data-t": "double", "{double}" }
        p { "data-t": "seen", "{log.peek().len()}" }
        button { "data-t": "theme", onclick: move |_| *THEME.write() = "dark", "{THEME}" }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(Counter);
ui.click("inc");
ui.click("inc");
assert_eq!(ui.text("double"), "6");
ui.click("theme");
assert_eq!(ui.read(&THEME), "dark");
# }
```

| Hook or item | Is | aio (AIR) |
|---|---|---|
| `use_signal(\|\| v)` | a value of this component; `n()` reads (a copy), `n.read()` borrows, `n.set(v)`, `n.write()`, `n += 1`, `n.toggle()` | `useLocal`, `useSignal` |
| `static S: GlobalSignal<T> = Signal::global(\|\| v)` | one value for the whole app (per app instance) | module-level `signal()` |
| `.peek()` | reads without following | `.peek()`, `untrack` |
| `use_memo(move \|\| ..)` | a derived value, re-made when what it reads changes; equal answers don't re-render | `computed()`, `useMemo` |
| `use_effect(move \|\| ..)` | runs after the render, again when what it reads changes | `effect()`, `useEffect` |
| `use_resource(move \|\| async move { .. })` | an async value, re-run when what it reads changes; `.read()` is `None` until ready | `useResource` |
| `use_hook(\|\| ..)` | made once, at the first render (mount) | `onMount`, `useRef` |
| `use_drop(\|\| ..)` | runs as the component goes (unmount) | `onCleanup`, `onUnmount` |
| `spawn(async move { .. })` | a task of the component, dropped with it | |
| `use_context_provider(\|\| v)`, `use_context::<T>()` | a value for every component inside | `createContext`, `useContext` |
| `aio::kit::use_context_select(\|t: &T\| ..)` | a part of a context, re-rendering only when it changes | `useContextSelector` |
| `aio::kit::use_id("tab")` | a stable, unique id for `aria-controls`, `for` | `useId` |
| `aio::kit::use_held(&pool, key)` | a held resource for this view, let go when the key changes or the view goes | `useResource` (a held one) |
| `aio::kit::use_frame(active, \|since_start, since_last\| ..)` | runs once a frame while `active` | `useRaf` |
| `aio::kit::use_interval(every, active, \|\| ..)` | a timer while `active` | `useInterval` |
| `aio::kit::use_window_event(\|e\| ..)` | the window's pointer, wheel, key and resize events (inside `GlobalKeys`) | `onWindowEvent` |
| `aio::kit::use_back_button(\|\| ..)` | takes Back while the view lives ([routing](routing.md#back)) | `onBackButton` |

### Lifecycle

| Moment | Write |
|---|---|
| first render (mount) | `use_hook(\|\| ..)`: runs once, its value kept |
| after each render that changed what it read | `use_effect(move \|\| ..)` |
| the element is in the page | `onmounted: move \|e\| ..` on it |
| a render | the component's body: keep it free of side effects |
| unmount | `use_drop(\|\| ..)`; tasks from `spawn` and async handlers stop by themselves |

Children render before their parents' effects run, so a parent's
`use_effect` sees its children mounted. Everything a component started
with `spawn`, `use_frame`, `use_interval` or a hook of the kit stops with
it: there is nothing to clean up by hand.

### Errors

A component may return an error with `?`; the nearest `ErrorBoundary`
shows its `handle_error` instead of the children that failed:

```rust
use dioxus::prelude::*;

#[component]
fn Amount(text: String) -> Element {
    let n: u32 = text.parse()?; // a parse error fails this render
    rsx! { p { "data-t": "amount", "{n}" } }
}

#[component]
fn App() -> Element {
    let mut text = use_signal(|| "12".to_string());
    rsx! {
        button { "data-t": "break", onclick: move |_| text.set("twelve".into()), "Break" }
        ErrorBoundary {
            handle_error: |_: ErrorContext| rsx! { p { "data-t": "oops", "Can't show that amount" } },
            Amount { text: text() }
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
assert_eq!(ui.text("amount"), "12");
ui.click("break");
assert!(ui.present("oops"));
# }
```

A panic in a render is a bug, not an error: a debug build stops on it,
and `TestUi` fails the test with the component named. An error in a
handler isn't caught by a boundary: a refused call through `act!` is
logged and gives `None`.

## Server state in a view

| Read | Gives | For |
|---|---|---|
| `part(\|c\| &c.field)` | one field, followed | everything shown |
| `use_status()` | `Status { online, pending, degraded, conn, last_sync, rtt }` | an offline banner |
| `aio::kit::ConnectionStatus { status: use_status()() }` | the kit's "Reconnecting…" pill | once, in the root |
| `use_pending(Some("save"))` | calls of `save` in flight | a spinner on its button |
| `use_time_travel()` + `aio::kit::TimeTravelPanel` | the cell's history, Ctrl+. to open | a dev panel |

A synced copy shows a call at once and confirms it with the server
later; a refusal rolls the view back ([sync](../state/sync.md)). There is
no separate optimistic layer to keep: the copy is it.

```rust
use aio::act;
use dioxus::prelude::*;

#[aio::cell(persist, id = "drafts")]
pub struct Drafts {
    pub saved: u32,
}

#[aio::methods]
impl Drafts {
    pub fn save(&mut self) {
        self.saved += 1;
    }
}

aio::client!(Drafts, drafts);

#[component]
fn App() -> Element {
    use_live();
    let status = use_status();
    let busy = use_pending(Some("save"));
    let saved = *part(|d| &d.saved);
    rsx! {
        aio::kit::ConnectionStatus { status: status() }
        button { "data-t": "save", disabled: busy() > 0, onclick: move |_| _ = act!(save()),
            if busy() > 0 { "Saving…" } else { "Save" }
        }
        p { "data-t": "saved", "{saved}" }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.click("save");
assert_eq!(ui.text("saved"), "1");
assert_eq!(ui.text("save"), "Save");
# }
```

## Coming from aio's AIR or React

| aio (AIR) | aio-rs (Dioxus) |
|---|---|
| `<li class="row" data-t="x">{name}</li>` | `li { class: "row", "data-t": "x", "{name}" }` |
| `{items.map(i => <Row key={i.id} />)}` | `for i in items.iter() { Row { key: "{i.id}" } }` |
| `{cond && <X />}` | `if cond { X {} }` |
| `onClick={() => ..}` | `onclick: move \|_\| ..` |
| `signal(0)` at module level | `static N: GlobalSignal<i32> = Signal::global(\|\| 0);` |
| `useLocal(0)` in a component | `let mut n = use_signal(\|\| 0);` |
| `computed(() => a.value * 2)` | `let double = use_memo(move \|\| a() * 2);` |
| `effect(() => …)` | `use_effect(move \|\| …)` |
| `onMount` / `onCleanup` | `use_hook(..)` / `use_drop(..)` |
| `useRef()` on an element | `onmounted: move \|e\| el.set(Some(e.data()))` |
| `batch(..)` | nothing: Dioxus renders once per tick |
| `untrack(..)` / `.peek()` | `.peek()` |
| `<Show when>` / `<For each>` | `if` / `for` inside `rsx!` |
| `ErrorBoundary` | `ErrorBoundary { handle_error: .., children }` |
| `afterRender(() => el.scrollIntoView())` | `"data-aio-reveal"` on the element, or `onmounted` |
| `useAio()` (all state) | `name()`, or better `part(..)` per field |
| `cell.field` read in a render | `part(\|c\| &c.field)` |
| `cell.method(x)` from a handler | `act!(method(x))` |
| `useConnected()` | `use_status()` |
| `cell.method.pending()` | `use_pending(Some("method"))` |
| `useHead({ title })` | `aio_shell::page::use_head(Head::title(..))` |
| `useState` / `useEffect` / `useMemo` (React) | `use_signal` / `use_effect` / `use_memo`, with no dependency arrays: they follow what they read |
| a React component | an island: `aio::kit::Island::react` ([islands](islands.md)) |

What changes in the way of thinking:

- **What re-renders.** AIR re-renders the components that read a signal;
  Dioxus re-renders a component when a signal it read changes or its
  props aren't equal, and its children only when their props changed.
  Rows by props, parts by field: [performance](../performance.md).
- **No dependency arrays.** Memos and effects follow what they read, as
  AIR's do; React's `[deps]` have no counterpart.
- **Types.** Props, routes, a cell's fields and its methods' arguments
  are checked by the compiler; a misspelt field is a build error, not
  `undefined`.

## Gaps against aio

- **Server rendering.** aio renders a page to HTML on the server
  (`renderToString`, `renderToStream`) and hydrates it; aio-rs's views
  run where the window is (the page, or the app process over its wire),
  so there is no HTML-first page to crawl.
- **Redux DevTools.** aio sends its actions to the browser extension;
  aio-rs has `use_time_travel()` with `kit::TimeTravelPanel`,
  `ax timetravel` and `ax events` instead.
- **`useOptimistic`.** None: a synced copy already shows a call at once
  and rolls a refusal back.
- **`watch()`, `on()`, `trackedMemo()`.** No separate forms: `use_effect`
  reading the value, or `use_memo`.
- **`h()` without JSX, the React compat layer.** `rsx!` only; React
  components come in as islands.

## The page

`aio_shell::page` has what a view asks of the page around it:

| Call | Does |
|---|---|
| `use_head(aio::head::Head::title("Inbox"))` | owns the window's title and `<meta>`/`<link>` tags while the component lives (a page's `routes!` variant gives its own: `page.head()`) |
| `reveal(selector, center)`, `reveal_id(id, center)` | scrolls an element into view |
| `set_scroll(selector, y)`, `caret(selector).await` | sets a scroll position, reads a field's caret |
| `copy(text).await`, `print()` | the clipboard, the print dialog |
| `pick(&Pick { .. }).await`, `pick_many`, `save(&Save { .. }).await` | the system's open and save dialogs |

`aio::kit::use_window_event(|e| ..)` hears the window's pointer, wheel, key
and resize events (pointer moves at most every `kit::MOVE_MS`, 100 ms),
under a `kit::GlobalKeys` around the app.

## The window

`aio_shell::run(App, aio_shell::Window { .. }, &dirs)` opens it.
`Window::of(&cfg)` takes the title, `lang` and `dir` from `[app]`; the
rest are fields:

| Field | Does |
|---|---|
| `width`, `height` | its first size; the size a person leaves it at is kept per app |
| `icon` | window and taskbar icon, PNG bytes (`include_bytes!`) |
| `background` | `[light, dark]` until the page paints |
| `viewport` | the `<meta name="viewport">`; `Some("")`: none |
| `tray` | a tray icon and its menu (`aio_shell::desktop::Tray`); its clicks reach the views through `desktop::use_asks` |
| `head` | the app's own `<head>` markup, after aio's policy, which it can only tighten |
| `web` | other sites inside the app: `browser` (the kit's `Browser`), `windows` (child windows), `preloads`, `fullscreen`, `downloads` |
| `permissions` | what the page may also hold (`"clipboard-read"`…) |
| `confirm_unload` | a page's `beforeunload` asks Leave / Stay |
| `keys` | the window's own keys (reload, print) given to the page |
| `require_sandbox` | no window rather than one without Chromium's sandbox: the app stops, saying why (aio's `requireSandbox`) |
| `show_cap_ms` | the longest wait for the page's first paint (5000 unset); `Some(0)` shows it at once |
| `keep_server` | the app serves on once the window closes, until asked to stop (aio's `keepServer`; `--keep-server` per launch) |
| `splash_show` | the app's say over `[build.splash]` at run time: `Always`, `FirstStart` or `Never` (aio's `splash.show`) |

- Notifications: `cx.tell(aio::tell::Tell::notify(..))` from a method
  ([effects](../state/effects.md)), or `aio_shell::desktop::notify(&Notice
  { title, body, route, .. })` from a view; a click brings the window
  forward and goes to `route`.
- Someone else's site: `aio::kit::Browser { src }` in a view (needs
  `web: Web { browser: true, .. }`), or a window of its own with
  `aio_shell::guest::open_window(&Child { url, .. })` (needs `windows:
  true`). Each runs sandboxed, in a session of its own, with only a preload
  the app declared.
- A piece of plain JavaScript inside a view (a chart library, a map):
  `aio::kit::WebIsland` ([kit](kit.md#islands-and-other-sites)).
- Size for a run only: `ax dev <app> --size 390x844`; a screenshot at a
  size: `ax shot <app> --size 390x844`.

Routes are [routing](routing.md); styles and dark mode are
[theming](theming.md).

## Common mistakes

- A hook behind an `if`, in a loop or after an early `return`: hooks are
  matched by call order, so the next render reads another hook's value.
  Call every `use_…` at the top, unconditionally.
- Side effects in the body of a component (a call, a write to a signal
  it reads): the body is a render and runs again; write in a handler or
  a `use_effect`.
- A signal read in a handler expecting the view to follow it: handlers
  don't subscribe; read it in the render.
- Reading `name()` in a render: it works, and re-renders the view on every
  commit of the cell. Read `part(..)` per field.
- `part(|c| &c.rows[i])`: a reference into a list can't name its field, so
  the view follows every commit. Pick the field, then index it.
- Forgetting `use_live()` in the root: views then miss commits that don't
  come from them (effects, sync, schedules).
- CSS or anything an edit should change kept in `use_hook`: it is made
  once, so a hot-patched edit doesn't show. Make it in the render.
- `e.prevent_default()` on keys in a window comes too late: put
  `data-aio-prevent` on the element ([kit](kit.md#keys)).
- A `for` loop whose body is a block keys each row inside an unkeyed
  wrapper: put `key:` on the element the loop yields (`ax lint` warns).

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::client!`,
`aio::act!`, `aio::ui` (`Part`, `cx`, `page_css`, `BACKGROUND`),
`aio::head`, `aio::kit`, `aio::testing::TestUi`; Dioxus's own hooks and
`rsx!` in `cargo doc -p dioxus --open`; `cargo doc -p aio-rs-shell --no-deps --open`: `Window`,
`page`, `desktop`, `guest`.

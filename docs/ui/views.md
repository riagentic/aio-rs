# Views

Views are `rsx!` components written against `aio::ui`
(`use aio::ui::prelude::*;`): real HTML and CSS, drawn by air, aio-rs's
own fine-grained renderer ([air](../design/air.md)). A component's body
runs once, as it mounts; what follows state is its holes (every text,
attribute, child, `if` and `for` of its `rsx!` that isn't a literal),
each writing only its own part of the page when what it read changes.
The same views are drawn by the shared Electron runtime on the
desktop, by the system WebView on Android and by the browser for
`ax build --web` ([targets](../build/targets.md)). A cell holds the state;
a view reads it and calls its methods. Views need aio-rs's feature `kit`
(the app template has it).

- **Use a view** for everything a person sees. Keep state the app must keep
  (a selection worth restoring, a draft) in a [cell](../state/cells.md);
  view-only state (hover, an open menu) in a view's signals.
- **Not a view**: a command-line tool ([cli apps](../clients/cli-apps.md))
  or a headless service (`ax create --template service`).

## A cell's client

```rust
use aio::act;
use aio::ui::prelude::*;

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
pub fn App() -> View {
    use_live(); // once, in the root: commits from effects reach the views too
    rsx! {
        main { Draft {} List {} }
    }
}

#[component]
fn Draft() -> View {
    let draft = part(|t| &t.draft); // a handle: a hole reading it follows `draft` only
    rsx! {
        form { onsubmit: move |e| { e.prevent_default(); _ = act!(add()); },
            input { "data-t": "draft", value: "{draft}",
                oninput: move |e| _ = act!(type_draft(e.value())) }
            button { r#type: "submit", disabled: draft.with(|d| d.trim().is_empty()), "Add" }
        }
    }
}

#[component]
fn List() -> View {
    let items = part(|t| &t.items);
    rsx! {
        ul { "data-t": "list",
            for (i, item) in items.get().into_iter().enumerate() {
                Item { key: "{i}", text: item }
            }
        }
    }
}

/// A row by props: kept by its key while the list around it changes.
#[component]
fn Item(text: String) -> View {
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
  a person clicks ─ handler ─ act!(add()) ─ the cell's method ─ commit ─ the holes that read
                                                                   │     a changed field run again
                                     effects, schedules, sync ─────┘     (at most every 150 ms)
```

- A **cell** owns the state and its rules; its methods are the only way
  to change it, and every change is an event in its log.
- A **view** reads the state in its holes (`part(..)`, `name()`), shows it, and turns what
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
| `part(\|c\| &c.field)` | a handle on one top-level field (`.get()`, `.with(\|v\| ..)`, `"{p}"`, `.ver()`); a hole reading it runs again only when it changes |
| `name()` | the whole state (`Arc<Cell>`); a hole reading it runs again on every commit; the body reads it once |
| `use_live()` | in the root, once: commits from effects, schedules and sync reach the views too |
| `use_status()` | how the copy stands: `online`, `pending`, `degraded`, `conn`, `last_sync`, `rtt` |
| `use_pending(Some("method"))` | calls of that method in flight (`None`: all), for a spinner |
| `use_time_travel()`, `time_travel(ask)` | the cell's history and steps through it (`kit::TimeTravelPanel`) |
| `act!(method(args))` | calls a method from a handler |

`aio::client!(Cell, name, name_at)` also makes `name_at(|c| value)`: a
[`memo`](#local-state-and-hooks) of a value made from the state, its
readers told when that value changes.

## Reading and calling

- `part(|c| &c.field)` is a handle on one top-level field; reading it
  in a hole or a `memo` (`.get()`, `.with(|v| ..)`, `"{p}"`) follows that
  field only. Read in the body (`let n = part(..).get();` before `rsx!`)
  it is read once, as the view mounts, and never follows. Pick the field, then index it (`part(|c| &c.rows).with(|r| r[i].clone())`,
  not `part(|c| &c.rows[i])`). `.ver()` is the field's stamp, for caches
  ([performance](../performance.md#1-read-fields)).
- `name()` reads the whole state. Use it in handlers, which don't
  follow anything; a dev build names, once, each view body that reads it
  (`[hint]` in the log).
- `act!(method(args))` calls the method; a refusal is logged and gives
  `None`, an answer `Some(value)`. A view's call shows at once; commits
  from effects, schedules and other threads reach the views at most once
  per `aio::ui::BACKGROUND` (150 ms).
- A refusal is the cell's last word, not the view's message: check the
  input in the view first (as `Draft` disables its button), so a person
  never meets a refusal they can't read.

## Components, props and children

A component is a function returning `View`; `#[component]` turns its
arguments into props. Its body runs once, and so are its props
evaluated, unless a prop is a `Prop<T>` (or a `Signal`, a `Part`): given
an expression (`value: name.get()`), a `Prop` follows it; given a
constant, it is that constant. Rows that take their data by props and
a key stay put when the list around them changes.

```rust
use aio::ui::prelude::*;

#[derive(Clone, PartialEq)]
struct Contact {
    id: u32,
    name: String,
    fav: bool,
}

/// `children` is what the caller puts inside; `note` may be left out.
#[component]
fn Panel(title: String, #[props(default)] note: Option<String>, children: View) -> View {
    rsx! {
        section { class: "card",
            h2 { "{title}" }
            {note.clone().map(|n| rsx! { p { class: "muted", "data-t": "note", "{n}" } })}
            {children}
        }
    }
}

#[component]
fn Contacts() -> View {
    let people = signal(|| {
        vec![Contact { id: 1, name: "Ada".into(), fav: true }, Contact { id: 2, name: "Linus".into(), fav: false }]
    });
    let field = signal(|| None::<std::rc::Rc<MountedData>>);
    let none = memo(move || people.with(Vec::is_empty));
    rsx! {
        Panel { title: "Contacts", note: "people you know",
            ul {
                for c in people.get() {
                    // key: what keeps a row's element (and its state) with its data
                    li { key: "{c.id}", class: aio::ui::cx(&[("row", true), ("fav", c.fav)]), "{c.name}" }
                }
            }
            if none.get() {
                p { "data-t": "empty", "No one yet" }
            }
            // A ref: the element, once mounted, for what only it can do.
            input { "data-t": "search", onmounted: move |e| *field.write() = Some(e.data()) }
            button { "data-t": "find",
                onclick: move |_| async move {
                    if let Some(el) = field.get() {
                        _ = el.set_focus(true).await;
                    }
                },
                "Find"
            }
            // `false` leaves a boolean attribute out.
            button { "data-t": "clear", disabled: none.get(), onclick: move |_| people.write().clear(), "Clear" }
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
| `value: Prop<String>` | follows the expression given (`value: name.get()`); `.get()` reads it in a hole |
| `note: Option<String>` | may be left out (`None`); `note: "x"` gives `Some` |
| `children: View` | what the caller writes inside |
| `onpick: EventHandler<String>` | a handler the caller passes; `onpick.call(v)` |
| `#[props(extends = GlobalAttributes)] attributes: Vec<Attribute>` | any HTML attribute, spread with `..attributes` |

### Markup

| Want | Write |
|---|---|
| text with values | `"{name} ({count})"` (format args; `{x:?}` too): a hole, following what it reads |
| a value that follows state | any expression in a text, attribute or `{child}`: `{n.get() * 2}`, `disabled: !ok.get()` |
| an attribute | `class: "row"`, `"data-t": "x"`, `"aria-label": "Close"`; any string (`String`, `&str`, `Cow`), number or `bool`: `title: t(&name.get())` |
| a value that may be absent | `title: tip` where `tip: Option<String>`: `None` leaves it out |
| classes by condition | `class: aio::ui::cx(&[("row", true), ("on", on)])` |
| inline style | `style: "width: {w}px"`, or one property: `width: "{w}px"` |
| a condition | `if on.get() { X {} } else { Y {} }`: swaps only when it flips |
| a value that may be absent | `if let Some(n) = note.get() { p { "{n}" } }`: made again when what it binds changes |
| a match | `match page.get() { Page::A => { A {} } Page::B(id) if id > 9 => p { "{id}" }, _ => {} }`: an arm's view is kept until another arm matches (one that binds names: until they change) |
| a repeat | `for`, never `while` or `loop` (refused): a view says what shows, not how long to repeat |
| a list | `for x in xs.get() { li { key: "{x.id}", .. } }` (`key:` on every row's first node: `rsx!` refuses a row without one; items are `Clone + PartialEq`) |
| several roots | a component may return them side by side: `rsx! { A {} B {} }` |
| an element made elsewhere | `{elem}` where `elem: View` |
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
  task of the component, dropped with it. A branch or a row of the component drawn
  again stops none of it.
- An event that reaches an element drawn again meanwhile (sent by the
  page before the redraw) is dropped; a dev build says so once per event
  name on the [diagnostic bus](../debugging/errors.md#hearing-problems-in-code) (`ui:stale-event`).
- `e.stop_propagation()` keeps the event from the elements around;
  `e.prevent_default()` from the browser, but keys in a window need
  `data-aio-prevent` ([kit](kit.md#keys)).
- A key that should work anywhere in the window is a global key
  (`aio::kit::use_global_key`), not an `onkeydown` on a `div`.

## Local state and hooks

State only a view needs (an open menu, a hover, a half-typed search)
lives in a view's signals; state the app keeps lives in a cell. What keeps
something for a view (`signal`, `memo`, `effect`, `watch`, `once`,
`on_cleanup`, `provide`, `callback`, `task`, `resource`, the kit's `use_…`)
is called at the top of a component: they are matched by the order they
are called in, so never call one behind an `if`, in a loop, or after an
early `return`.

```rust
use aio::ui::prelude::*;

/// One per app instance (per test), shared by every component.
static THEME: Global<&'static str> = Global::new(|| "light");

#[component]
fn Counter() -> View {
    let mut n = signal(|| 1);
    let double = memo(move || n.get() * 2); // follows `n`
    let log = signal(Vec::<i32>::new);
    // Runs once the view is drawn, again when what it reads changes.
    effect(move || log.write().push(double.get()));
    rsx! {
        button { "data-t": "inc", onclick: move |_| n += 1, "{n}" }
        p { "data-t": "double", "{double}" }
        p { "data-t": "seen", "{log.read_untracked().len()}" }
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

| Item | Is | aio (AIR) |
|---|---|---|
| `signal(\|\| v)` | a `Copy` value of this component; `n.get()` reads (a copy), `n.with(\|v\| ..)` and `n.read()` borrow, `n.set(v)` (readers told only if it differs), `n.update(\|v\| ..)`, `n.write()`, `n += 1`, `n.toggle()`, `"{n}"` | `useLocal`, `useSignal` |
| `static S: Global<T> = Global::new(\|\| v)` | one value for the whole app (per app instance), the same methods | module-level `signal()` |
| `.get_untracked()`, `.with_untracked(..)`, `.read_untracked()`, `untrack(\|\| ..)` | reads without following | `.peek()`, `untrack` |
| `memo(move \|\| ..)` | a derived value, re-made when what it reads changes; equal answers wake no reader | `computed()`, `useMemo` |
| `effect(move \|\| ..)` | runs once the view is drawn, again when what it reads changes, its own writes too; returns an `Effect`, `e.dispose()` stops it | `effect(fn, { rerunOnSelfWrite: true })`, `useEffect` |
| `effect_quiet(move \|\| ..)` | `effect`, but a signal it read and then wrote itself doesn't run it again, so `log.update(\|l\| l.push(x.get()))` can't loop | `effect()` |
| `resource(move \|\| async move { .. })` | an async value, re-run when what it reads changes; `.get()` is `None` until ready, `.suspend()?` waits under a `Suspense`; `.restart()`, `.mutate(v)` shows `v` at once (the next load replaces it), `.dispose()` loads no more | `useResource`, `resource()` |
| `watch(deps, move \|deps\| ..)` | runs once the view is drawn, again when `deps` (values the view made, e.g. of its props) differ | |
| `on_change(move \|\| .., \|next, prev\| ..)` | runs when the first closure's value changes (not at the start), with the old one; only what it reads is followed, the second runs untracked | `watch(source, fn)`, `effect(on(..))` |
| `callback(move \|x\| ..)` | a handler made once, for a prop | `useCallback` |
| `task(move \|\| async move { .. })` | work for the view's life: started at mount, stopped at unmount | `onMount` + a loop |
| `once(\|\| ..)` | made once, as the view mounts | `onMount`, `useRef` |
| `on_cleanup(\|\| ..)` | runs as the component goes (unmount) | `onCleanup`, `onUnmount` |
| `spawn(async move { .. })` | a task of the component, dropped with it (in a handler; in the body, `task`) | |
| `spawn_forever(async move { .. })` | a task of the app, outliving the view that started it | |
| `provide(\|\| v)`, `context::<T>()`, `try_context::<T>()` | a value for every component inside | `createContext`, `useContext` |
| `eval(js)` | a script in the page: `aio.send(v)` hands a value to the view (`.recv().await`), `return v` ends it (`.join().await`) | |
| `aio::kit::use_context_select(\|t: &T\| ..)` | a part of a context, followed only when it changes | `useContextSelector` |
| `aio::kit::use_id("tab")` | a stable, unique id for `aria-controls`, `for` | `useId` |
| `aio::kit::use_held(&pool, key)` | a held resource for this view, let go when the key changes or the view goes | `useResource` (a held one) |
| `aio::kit::use_frame(active, \|since_start, since_last\| ..)` | runs once a frame while `active` | `useRaf` |
| `aio::kit::use_interval(every, active, \|\| ..)` | a timer while `active` | `useInterval` |
| `aio::kit::use_window_event(\|e\| ..)` | the window's pointer, wheel, key and resize events (inside `GlobalKeys`) | `onWindowEvent` |
| `aio::kit::use_window_listener(kind, \|e\| ..)` | any event type the window hears, each one, as JSON | `onWindowEvent(type)` |
| `aio::kit::use_back_button(\|\| ..)` | takes Back while the view lives ([routing](routing.md#back)) | `onBackButton` |

### Lifecycle

| Moment | Write |
|---|---|
| mount | the component's body (once); `once(\|\| ..)`: runs once, its value kept |
| when what it read changed | `effect(move \|\| ..)` |
| the element is in the page | `onmounted: move \|e\| ..` on it |
| a change | the holes that read it: keep them free of side effects |
| unmount | `on_cleanup(\|\| ..)`; tasks from `spawn` and async handlers stop by themselves |

Children are drawn before their parents' effects run, so a parent's
`effect` sees its children mounted. Everything a component started
with `spawn`, `task`, `use_frame`, `use_interval` or a hook of the kit stops with
it: there is nothing to clean up by hand.

### Errors

A component may return an error with `?`; the nearest `ErrorBoundary`
shows its `fallback` (given the error's text) instead of the children
that failed:

```rust
use aio::ui::prelude::*;

#[component]
fn Amount(text: String) -> View {
    let n: u32 = text.parse()?; // a parse error fails this view
    rsx! { p { "data-t": "amount", "{n}" } }
}

#[component]
fn App() -> View {
    let broken = signal(|| false);
    rsx! {
        button { "data-t": "break", r#type: "button", onclick: move |_| broken.set(true), "Break" }
        ErrorBoundary {
            fallback: |_: String| rsx! { p { "data-t": "oops", "Can't show that amount" } },
            if broken.get() {
                Amount { text: "twelve" }
            } else {
                Amount { text: "12" }
            }
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

The fallback may try again (aio's boundary reset): `retry()` in its body
gives the boundary's `Retry`, and `r.retry()` makes the children anew;
one that fails again shows the fallback again.

```rust
use aio::ui::prelude::*;

#[component]
fn Retrying() -> View {
    rsx! {
        ErrorBoundary {
            fallback: |e: String| {
                let r = retry();
                rsx! { button { "data-t": "again", onclick: move |_| r.retry(), "Try again ({e})" } }
            },
            Amount { text: "twelve" }
        }
    }
}
# #[component]
# fn Amount(text: String) -> View {
#     let n: u32 = text.parse()?;
#     rsx! { p { "{n}" } }
# }
# fn main() {
let mut ui = aio::testing::TestUi::new(Retrying);
ui.click("again");
assert!(ui.present("again"), "still failing: the fallback again");
# }
```

A view waiting for a [`resource`](#local-state-and-hooks) (`res.suspend()?`)
shows the nearest `Suspense { fallback: rsx! { .. }, .. }`'s fallback until
it loads. A panic in a view is a bug, not an error: a debug build stops on it,
and `TestUi` fails the test with the component named. An error in a
handler isn't caught by a boundary: a refused call through `act!` is
logged and gives `None`.

## Server state in a view

| Read | Gives | For |
|---|---|---|
| `part(\|c\| &c.field)` | a handle on one field, followed | everything shown |
| `use_status()` | `Status { online, pending, degraded, conn, last_sync, rtt }` | an offline banner |
| `aio::kit::ConnectionStatus { status: use_status().get() }` | the kit's "Reconnecting…" pill | once, in the root |
| `use_pending(Some("save"))` | calls of `save` in flight | a spinner on its button |
| `use_time_travel()` + `aio::kit::TimeTravelPanel` | the cell's history, Ctrl+. to open | a dev panel |

A synced copy shows a call at once and confirms it with the server
later; a refusal rolls the view back ([sync](../state/sync.md)). There is
no separate optimistic layer to keep: the copy is it.

```rust
use aio::act;
use aio::ui::prelude::*;

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
fn App() -> View {
    use_live();
    let status = use_status();
    let busy = use_pending(Some("save"));
    let saved = part(|d| &d.saved);
    rsx! {
        aio::kit::ConnectionStatus { status: status.get() }
        button { "data-t": "save", r#type: "button", disabled: busy.get() > 0, onclick: move |_| _ = act!(save()),
            if busy.get() > 0 { "Saving…" } else { "Save" }
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

| aio (AIR) | aio-rs (`aio::ui`) |
|---|---|
| `<li class="row" data-t="x">{name}</li>` | `li { class: "row", "data-t": "x", "{name}" }` |
| `{items.map(i => <Row key={i.id} />)}` | `for i in items.get() { Row { key: "{i.id}" } }` |
| `{cond && <X />}` | `if cond.get() { X {} }` |
| `onClick={() => ..}` | `onclick: move \|_\| ..` |
| `signal(0)` at module level | `static N: Global<i32> = Global::new(\|\| 0);` |
| `useLocal(0)` in a component | `let n = signal(\|\| 0);` |
| `computed(() => a.value * 2)` | `let double = memo(move \|\| a.get() * 2);` |
| `effect(() => …)` | `effect(move \|\| …)` |
| `onMount` / `onCleanup` | `once(..)` / `on_cleanup(..)` |
| `useRef()` on an element | `onmounted: move \|e\| *el.write() = Some(e.data())` |
| `batch(..)` | nothing: a handler's writes reach the page in one flush |
| `untrack(..)` / `.peek()` | `.get_untracked()`, `.read_untracked()` |
| `<Show when>` / `<For each>` | `if` / `for` inside `rsx!` |
| `ErrorBoundary` | `ErrorBoundary { fallback: \|e: String\| rsx! { .. }, children }` |
| `<Suspense>` | `Suspense { fallback: rsx! { .. }, children }` |
| `afterRender(() => el.scrollIntoView())` | `"data-aio-reveal"` on the element, or `onmounted` |
| `useAio()` (all state) | `name()`, or better `part(..)` per field |
| `cell.field` read in a component | `part(\|c\| &c.field)`, read in a hole |
| `cell.method(x)` from a handler | `act!(method(x))` |
| `useConnected()` | `use_status()` |
| `cell.method.pending()` | `use_pending(Some("method"))` |
| `useHead({ title })` | `aio_shell::page::use_head(Head::title(..))` |
| `useState` / `useEffect` / `useMemo` (React) | `signal` / `effect` / `memo`, with no dependency arrays: they follow what they read |
| a React component | an island: `aio::kit::Island::react` ([islands](islands.md)) |

What changes in the way of thinking:

- **What runs again.** AIR re-renders the components that read a
  signal; air runs a component's body once, and what read a changed
  signal runs again: a hole (a text, an attribute, a child, an `if`, a
  `for`), a `memo` or an `effect`, never the body.
  State read once at the top of the body is a snapshot; props follow only
  as `Prop<T>`. Rows by props, parts by field: [performance](../performance.md).
- **No dependency arrays.** Memos and effects follow what they read, as
  AIR's do; React's `[deps]` have no counterpart.
- **Types.** Props, routes, a cell's fields and its methods' arguments
  are checked by the compiler; a misspelt field is a build error, not
  `undefined`.

## Gaps against aio

- **Server rendering.** aio renders a page to HTML on the server
  (`renderToString`, `renderToStream`) and hydrates it; aio-rs's views
  run where the window is (the page, or the app process over its wire),
  so there is no HTML-first page to crawl. By design: a desktop or
  Android page is local and draws its first frame from memory, and no
  hydration means no mismatch between two renders; a public page's HTML
  for crawlers and previews is `ax preview --html` and `aio::head::html`.
- **Redux DevTools.** aio sends its actions to the browser extension;
  aio-rs has `use_time_travel()` with `kit::TimeTravelPanel`,
  `ax timetravel` and `ax events` instead: no browser extension, every
  target, release builds too, and the whole store rather than 2000 entries.
- **`useOptimistic`.** None: a synced copy already shows a call at once
  and rolls a refusal back.
- **`trackedMemo()`.** aio's cache of derived values by key, shared by
  components: a `memo` per component, or a `GlobalMemo` per value, each
  following what it reads.
- **`h()` without JSX, the React compat layer.** `rsx!` only; React
  components come in as islands. One renderer keeps events, focus, the
  audits and `TestUi` paths the same on every page.

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
| `web` | other sites inside the app: `browser` (the kit's `Browser`), `windows` (child windows), `preloads`, `fullscreen`, `downloads`, `unsandboxed_windows` |
| `permissions` | what the page may also hold (`"clipboard-read"`…) |
| `confirm_unload` | a page's `beforeunload` asks Leave / Stay |
| `keys` | the window's own keys (reload, print) given to the page |
| `require_sandbox` | no window rather than one without Chromium's sandbox: the app stops, saying why (aio's `requireSandbox`) |
| `show_cap_ms` | the longest wait for the page's first paint (5000 unset); `Some(0)` shows it at once |
| `keep_server` | the app serves on once the window closes, until asked to stop (aio's `keepServer`; `--keep-server` per launch) |
| `splash_show` | the app's say over `[build.splash]` at run time: `Always`, `FirstStart` or `Never` (aio's `splash.show`) |
| `devtools` | a build outside `ax dev` has DevTools too: F12, Ctrl+Shift+I and the launch's `--cdp=<port>`; off, an `ax dev` build's only |

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

- State read at the top of the body (`let n = part(..).get();`, `let s =
  name();`, then `"{n}"`): read once, as the view mounts, it never
  follows. Read it where it is shown (`"{n}"` with `let n = part(..)`,
  `{name().x}`, `n.get()` inside `rsx!`) or in a `memo`.
- A prop that must follow state given as a plain value (`text: String`
  passed `text.get()`): props are evaluated once. Type it `Prop<T>`, or
  pass a `Signal` or a `part` handle.
- A `signal`, `memo`, `effect`, `once` or `use_…` behind an `if`, in a loop
  or after an early `return`: they are matched by call order (a hot
  reload and a body run again after a `Suspense` find them by it), so
  another one's value comes back. Call them at the top, unconditionally
  (`ax lint` refuses it).
- A signal read in a handler expecting the view to follow it: handlers
  don't subscribe; read it in a hole.
- Reading `name()` in a hole: it works, and runs the hole again on every
  commit of the cell. Read `part(..)` per field.
- `part(|c| &c.rows[i])`: a reference into a list can't name its field, so
  the hole follows every commit. Pick the field, then index it.
- Forgetting `use_live()` in the root: views then miss commits that don't
  come from them (effects, sync, schedules).
- CSS or anything an edit should change kept in `once`: it is made
  once, so a hot-patched edit doesn't show. Make it in a hole.
- `e.prevent_default()` on keys in a window comes too late: put
  `data-aio-prevent` on the element ([kit](kit.md#keys)).
- A `for` row without `key:` on its first node: `rsx!` refuses it.
- A 0.1 app's renderer left in (its paths, its dependency, a page
  script's `send`): `ax lint` refuses them, and the migration guide's
  section 8, [0.1 → 0.2](../migration/README.md), has the names.

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::client!`,
`aio::act!`, `aio::ui` (`Part`, `cx`, `page_css`, `BACKGROUND`),
`aio::head`, `aio::kit`, `aio::testing::TestUi`, and the view API in
`aio::ui::prelude`; `cargo doc -p aio-rs-shell --no-deps --open`: `Window`,
`page`, `desktop`, `guest`.

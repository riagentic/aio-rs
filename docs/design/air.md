# air: aio-rs's own renderer (0.2.0 spike)

Status: step 1 is on main: air's core (`crates/air`, `crates/air-macro`),
its gates 1–3, and the facade and `TestUi` on it behind aio-rs's feature
`air` ([Phase 1](#phase-1-what-it-delivered)). Dioxus stays the default
renderer until step 5; with `kit` on too the facade is Dioxus's (the kit is
Dioxus code until step 2), so `--all-features` builds what ships today.
The spike (branch `air`, from v0.1.0) decided the design with numbers;
its bench (`crates/air-bench`, `bench.sh`) is on main too. Step 0, the
facade apps are written against (`aio::ui::prelude`): see
[The app surface](#the-app-surface-aioui).

## Verdict

**Proceed, adjusted.** On the same views air expands to 5.8x less code
and 3.5x less crate metadata than Dioxus, rebuilds a 50-view crate after a
one-line edit in half the time (1.1 s vs 2.2 s), builds clean in a third
of the time with 6 crates instead of 77, and answers a held ↓ over 1000
rows in about half the time (0.7 vs 1.3 ms p50) with 27 % fewer bytes;
over 10 000 rows it stays at 1.0 ms where Dioxus grows to 5.2 ms.

Adjustments the spike forced or found:

- **Holes take boxed closures, never generics.** The first prototype's
  generic holes made 3.7x Dioxus's LLVM IR, a 168 MB debug binary and a
  21.6 s rebuild; boxed: 0.32x the IR, 18 MB, 1.1 s. A rule for all of
  air's runtime API and handlers too, kept by `scripts/air-ir` in
  `ci.sh` (LLVM IR lines per view of the bench's 50 views, limit 25 000).
- **Views need live ranges** where a row's root is a hole (a component
  whose root is a `for`): the spike's range is fixed at build.
- **`rsx!`'s clone heuristic** (which names a hole reads, so it can
  clone them in) is the riskiest piece: it guesses from tokens. 0.2.0
  keeps it, with clear compile errors and an escape (write the
  `move ||` yourself).
- **Gate it**: one renderer per release; if air isn't through the gates
  by the 0.2.0 freeze, 0.2.0 ships on Dioxus behind the facade.

## Why

aio-rs 0.1.0 draws Dioxus 0.7 views as liveview: a Rust VirtualDom diffs,
edit bytes go over the window's socket, the shared Electron page applies
them. Runtime speed is fine. What hurts:

- a changed view re-runs whole and its dynamic nodes are diffed;
- Dioxus signals are a second state system beside aio cells;
- the renderer can't be fixed or tuned by us;
- `rsx!`'s expansion is huge: a 74 000-line app expands to about 60 MB
  of code and 44 MB of crate metadata, the main reason an edit in a big
  crate takes about 10 s.

aio (the TypeScript framework aio-rs succeeds) dropped React for its own
renderer, AIR, for the same reasons. air is the aio-rs port of that move.

## What air is

- **A view runs once.** Its body makes its state and returns a tree; the
  tree is built where it is placed (under that place's owner, so its
  context and its error boundary are the place's), cloning its templates
  and wiring its holes.
- **A hole is an effect.** A text, an attribute, a child, a branch or a
  list subscribes to exactly the signals it read and writes its own DOM
  node when one changes. No re-run, no VDOM, no diff (only a hole's last
  value and a list's keys are compared).
- **Cell fields are signals.** `part(|c| &c.count)` is a `Copy` handle; a
  hole reading it re-runs only when that top-level field changes
  (`changed_fields`, the mask aio-rs already computes per commit).
- **The macro is lean.** `rsx!` keeps Dioxus's syntax, emits one `static`
  template per element tree (its markup pre-encoded: one byte-string
  token) and one call per hole. No syn: the input is split on token trees
  and expressions pass through with their spans.
- **The wire stays.** The same socket, 4-byte frames, `aio:wire` IPC and
  `window.__aioWire` preload; only the page's interpreter changes (Dioxus's
  glue → `air::PAGE_JS`, 5.7 KB unminified).

Code: `crates/air` (aio-rs-air: reactive core, DOM holes, events, tasks,
the Rust test page, `page.js`), `crates/air-macro` (aio-rs-air-macro:
`rsx!`, `#[component]`, `#[derive(Props)]`; deps proc-macro2 + quote,
already in the graph), `aio::ui` on air (feature `air`:
`crates/aio/src/ui/on_air.rs`), measurements in `crates/air-bench`
(`bench.sh`). air's one dependency is `keyboard-types` (the W3C key
tables Dioxus uses too).

### What's copied from aio's AIR

AIR itself is a VDOM renderer with signal fast paths (components re-run on
a signal they read and are diffed, `aio/src/air/renderer-rerender.ts:218`,
`vdom-diff.ts`); only a signal passed raw as a child or prop is bound
straight to the DOM (`vdom-helpers.ts:44` `_bindSignalText`,
`signal-binding.ts:52` `bindSignalProps`). air makes that fast path the
only path, decided at compile time. Copied as is:

| air | from AIR |
|---|---|
| `Signal::set` notifies only on a changed value | `state/signal.ts:646` (`Object.is`) |
| `batch`, writes flush once at its end; handlers run in one | `signal.ts:396`, `air/vdom-events.ts:251` `_wrapHandler` |
| `untrack` | `signal.ts:316` |
| flush: queued effects deduped, owners first, a re-run guard | `signal.ts:451` `_flushSubscribers`, its 1000-round guard :418 |
| an effect's re-run disposes what its last run made | `_effectCollectStart` `signal.ts:1068` (as an owner tree) |
| `on_cleanup` | `air/renderer-lifecycle.ts:116` |
| keyed list: the longest increasing run of old places stays, the rest move | `air/vdom-diff-children.ts:296` `_longestIncreasing`, :321 `diffKeyed` |
| one listener per event type on the document | `air/vdom-events.ts:97` `_ensureDelegation` |
| a handled `submit` never navigates | `vdom-events.ts` (the SPA default) |
| `show(cond, then, else)` | `air/show.ts:9` `Show` |
| `memo` | `signal.ts:971` `computed` |

Not copied: component re-execution, the VDOM diff, auto-memo of props
(`renderer-rerender.ts:847`), the 12 ms render budget
(`renderer-flush.ts:227`: nothing re-renders, so nothing to budget), and
AIR's wire (it ships state patches to a browser renderer; aio-rs keeps
the renderer in Rust and ships DOM edits, as today).

New in air (from Solid): `selector` (`createSelector`): a cursor over 1000
rows re-runs two holes, not 1000.

Not AIR's, on purpose: an event goes to Rust with every node on its path
that listens for it, innermost first, and the handlers run in that order
until one calls `stop_propagation` (AIR's innermost handler alone wins;
the views were written against Dioxus's bubbling). Two rows of a list
with one key fail in every build (aio's tripwire), where AIR made the
second fresh.

## API (real code, from the prototype's tests)

A counter on an aio cell: the template app's window
(`crates/aio/tests/air_cells.rs`):

```rust
#[aio::cell(id = "counter")]
pub struct Counter { pub count: i64, pub label: String }

#[aio::methods]
impl Counter {
    pub fn add(&mut self, by: i64) { self.count += by; }
    pub fn reset(&mut self) { self.count = 0; }
}

aio::air_client!(Counter, counter);

pub fn app() -> View {
    let n = part(|c| &c.count);
    rsx! {
        main {
            h1 { "data-t": "count", "{n}" }
            button { class: aio::ui::btn("primary"), onclick: move |_| _ = act!(add(1)), "Add" }
            button { class: aio::ui::btn("ghost"), disabled: n.get() == 0, onclick: move |_| _ = act!(reset()), "Reset" }
        }
    }
}
```

A click on Add sends one 10-byte edit (`SET_TEXT` of the `h1`'s text
node); a call that changes `label` runs no hole that reads `count`.

A filtered, keyed list of 1000 rows with a cursor (`crates/air/tests/views.rs`):

```rust
fn list() -> View {
    let all = Signal::new(rows(1000));
    let filter = Signal::new(String::new());
    let cursor = Signal::new(0u32);
    let sel = selector(move || cursor.get());
    let shown = air::memo(move || {
        let f = filter.get();
        all.with(|r| r.iter().filter(|r| r.name.contains(f.as_str())).cloned().collect::<Vec<_>>())
    });
    rsx! {
        input { value: filter.get(), oninput: move |e: FormEvent| filter.set(e.value()) }
        div { tabindex: "0",
            onkeydown: move |e: KeyboardEvent| match e.key() {
                Key::ArrowDown => cursor += 1,
                Key::Character(c) if c == "r" => all.update(|r| r.reverse()),
                _ => {}
            },
            ul {
                for row in shown.get() {
                    li { key: row.id, class: if sel.is(row.id) { "row on" } else { "row" }, "{row.name}" }
                }
            }
        }
    }
}
```

Reversing moves 999 rows and makes none; typing a filter removes and
re-inserts rows by key; ↓ rewrites two `class` attributes (48 B a key).

A form:

```rust
fn form() -> View {
    let name = Signal::new(String::new());
    let agreed = Signal::new(false);
    let sent = Signal::new(Vec::<String>::new());
    rsx! {
        form { onsubmit: move |_| sent.update(|s| s.push(name.get_untracked())),
            input { value: name, oninput: move |e| name.set(e.value()) }
            input { r#type: "checkbox", checked: agreed.get(), onchange: move |e| agreed.set(e.checked()) }
            p { if name.with(|n| n.is_empty()) { "Who are you?" } else { "Hello, {name}!" } }
            button { disabled: !agreed.get(), "Send" }
            ul { for (i, s) in sent.get().into_iter().enumerate() { li { key: i, "{s}" } } }
        }
    }
}
```

A component with props and children:

```rust
#[component]
fn Card(title: String, count: Signal<u32>, children: View) -> View {
    rsx! {
        section { class: "card",
            h2 { "{title} ({count})" }
            {children}
        }
    }
}

// used as
rsx! {
    if open.get() {
        Card { title: "Cart", count: n,
            p { "{n} items" }
            for i in 0..n.get() { span { key: i, "·" } }
        }
    } else {
        p { "closed" }
    }
}
```

`#[component]` makes `#[derive(Clone)] CardProps { pub title, pub count,
pub children }` and `fn Card(CardProps { .. }: CardProps)`; `Card { … }`
in `rsx!` is a component hole running `Card(CardProps { title: "Cart".into(),
… })` once, untracked. `children` is a `View` built inside `Card`'s
template, under `Card`: it sees `Card`'s context. Toggling `open` leaves
nothing behind (`air::live()`, the test page's `named`: gate 1).

### The rules `rsx!` follows

- A literal (`"Add"`, `class: "row"`, `disabled: true`) goes in the template.
- A `"…{x}…"` string, and every other expression in a text, attribute or
  `{child}`, is a hole: `move || expr`, run once and again when what it
  read changes. The local names it uses are cloned in first (a heuristic
  over tokens: fields, methods, paths, calls, types, closure params, `let`
  and match patterns are not names to clone), so a `String` used by two
  holes needs no `.clone()`. A bare name is cloned per run; its type
  decides whether it follows anything (a `Signal` or `Part` does, a
  `String` doesn't). A name called (`f(x)`) is a function's, never
  cloned in (cloning a generic fn item fixed its type for every hole), so
  a local closure called in a hole is a `Callback`, or cloned in by a
  `let` of the hole's own; `aio::field!(name)` names a field, not a local.
- `onX: handler` listens for `X` (Dioxus's event names).
- `for pat in expr { row }` is a keyed list: `key:` on the row is required.
  Items must be `Clone + PartialEq`: a key whose item changed gets a new row.
- `if cond { … } else { … }` swaps only when `cond` flips (`memo`).
  `if let` is refused: make a bool, or use a `{child}` hole of an `Option<View>`.
- `onX` handlers are typed by Dioxus's event table (`onkeydown` gets a
  `KeyboardEvent`, `oninput` a `FormEvent`, …); an unknown `onX` is a
  compile error; `onmounted` runs once the element is drawn.
- A component's props are evaluated once: a prop that must follow state is
  a `Signal`, a `Part` or typed `Prop<T>` (as in Solid and Leptos): a
  `Prop` given an expression (`value: name.get()`) follows it, given a
  constant it is that constant. `#[props(default)]`, `#[props(default =
  e)]`, `Option<T>` and `View` props may be left out (an `Option` takes a
  `T`); any other prop left out is a compile error. A `View` prop takes a
  view or text (a string, a number, a format string, a signal), and text
  that reads state follows it (`air::text_view`). Unknown names given
  to a component with `#[props(extends = GlobalAttributes)] attributes:
  Vec<Attribute>` are attributes (`..attributes` on an element sets them,
  after its own).
- A body runs once, untracked; one that `?`s a resource still loading
  runs again when it is loaded, its hooks (`signal`, `memo`, `resource`,
  `once`, …) handing back what the first run made, by call order.
- A `View` is built where it is placed, and again each time it is placed
  (a dialog opened again, a tab shown again, one view in two places): each
  place gets its own build and state, as a Dioxus `Element` rendered again.
- A view is a range of siblings, first to last; a template starting with a
  hole gets an empty comment as its start, so a hole's content at a root
  moves and goes with its view.

## How cells bind

`client!(Cell, read)` is the same macro on both renderers: it is written
against the facade's names. It keeps, per app instance (globals), the
state the views were last told of and one `Signal<u64>` per top-level
field (`CellData::FIELDS`, `aio::ui::Parts`). `part(f)` finds the field
`f` points into by offset and returns a `Part`. Reading it in a hole
tracks that field's signal. After a call through the views' door
(`act!`), `tell()` diffs the new `Arc<State>` with `changed_fields` and
bumps the changed fields' signals: exactly the holes that read them
re-run. Commits made off the UI thread (effects, sync) reach the views
through `use_live`'s task, at most once per `BACKGROUND`. There is no
second store: view-local state is a plain `Signal`, everything kept is a
cell.

## The wire

Rust → page: one binary frame per flush, little-endian, ops back to back.
Strings are `u32` length + UTF-8. Ids are `u32`, allocated by the Rust
side; node 0 is `#main`.

| op | name | body |
|---|---|---|
| 1 | DEFINE | tpl, roots, count, nodes (pre-order: `1 tag nattrs (k v)* children:u16 mark:u8` element, `2 str` text, `3` text hole, `4` anchor) |
| 2 | CLONE | tpl, base: the marked nodes (roots, holes, elements with holes) get `base..` in pre-order |
| 3 | SET_TEXT | id, str |
| 4 / 5 | SET_ATTR / REMOVE_ATTR | id, name (, value) |
| 6 / 7 | SET_PROP / SET_BOOL | id, name, str / u8 (`value`, `checked`, `selected`) |
| 8 | LISTEN | id, event name |
| 9 / 10 | INSERT / APPEND | anchor or parent, first, last (a view: a sibling range) |
| 11 | REMOVE | first, last (its named nodes forgotten) |
| 12 | TEXT | id, str (a new text node, for a `{child}` that is text) |
| 13 | PARK | first, last (moved off the page, still named: a `Suspense`'s hidden side) |
| 14 | EVAL | script id, code (run as a script of the page's own making, `aio::page::RUN`) |
| 15 | EVAL_SEND | script id, JSON (its `await aio.recv()`) |

A template is sent once per page, cloned with `cloneNode` (which keeps
empty text holes). A `value` property set by the views is skipped while
it is an older echo of what the person typed (aio's typing rule, in
`page.js`). Page → Rust, one frame each, its first byte its kind:
`1` an event (`air::Raw`: the listening ids on its path, innermost
first; its name, key, code, location, modifier and state flags, mouse
button and buttons, client/page/offset/screen points, wheel deltas and
mode, the target's value and scroll box, pointer id and type, a
submitted form's fields), `2` a script's reply (id, kind: a value, its
return or its failure, JSON).

## Hot reload and hot patch

- **Templates are data.** A literal-only edit (text, class, a static
  attribute, static structure) changes only a template's bytes. Plan:
  air-macro's parser becomes a library (`air-rsx`) that `ax dev` runs on
  the saved file; if the holes are the same, the new bytes go to the app
  over the dev door, keyed by `file:line:col`, `tpl()` takes the override,
  and the views are remounted. Nothing compiles.
- **Rust edits: subsecond.** The standalone `subsecond` crate already
  patches cell methods in 0.1.0 (`aio/src/store.rs:783` `HotFn`), built by
  `dx` as today. A view is not re-run after a patch (it runs once), so on
  `subsecond::register_handler` air disposes the root and mounts
  `HotFn::current(app)` again: cells keep their state, view-local signals
  start again (Dioxus keeps hook state across a patch; that is lost).
  The remount costs a first mount: 1.4 ms for the 1000-row list, 8.3 ms for 10 000 rows (release, the
  cell's fill included; `hold`'s `air mount` line).
- Not measured in the spike (e): building a patch needs `dx`'s linker
  interception, which `ax dev` drives for Dioxus's crates today; wiring it
  to air is plan step 4.

## Web (wasm) and Android

- **Web**: air has no dependency and no threads; `page.js` takes a frame
  as a `Uint8Array`, so the wasm build calls `__airApply` with a view of
  its own memory per flush (no copy, no socket) and events call a wasm
  export instead of `__aioWire.send`. One interpreter for both. The web
  build's server-rendered first paint (`aio-shell/web`, `dioxus-ssr`) is
  dropped (out of scope: SSR).
- **Android**: unchanged in shape: the Rust runtime in the app process,
  the system WebView's page over the existing bridge (`window.__aioWire`
  defined by the page, `aio-shell/src/android.rs`). Only the page body
  changes.

## Measurements

Same machine (32 threads, Linux), other sessions' load present (load
average about 4), 3 runs each; `crates/air-bench/bench.sh` reproduces all
of it. The apps: 1 and 50 views of 204 rsx lines each (40 blocks of a
static label, a class and a text that follow a signal, a click handler;
a keyed loop; a branch), the same in Dioxus 0.7.10 and air, each a lib
(the views) + bin crate with aio-rs's dev profile.

| measure | Dioxus 0.7.10 | air | air / Dioxus |
|---|---|---|---|
| (a) `cargo expand` of the lib, 1 view | 385 KB | 65 KB | 0.17 |
| (a) the same, 50 views | 18.9 MB | 3.2 MB | 0.17 |
| (a) per view ((50 − 1) / 49) | 378 KB | 65 KB | 0.17 |
| (a) crate metadata (`.rmeta`), 50 views | 6.18 MB | 1.75 MB | 0.28 |
| (a) LLVM IR lines (debug), 50 views | 3.65 M | 1.17 M | 0.32 |
| (a) debug binary, 50 views | 53.2 MB | 18.4 MB | 0.35 |
| (b) one-line edit → `cargo build` (lib + link), 3 runs | 2.2 / 2.3 / 2.1 s | 1.1 / 1.0 / 1.1 s | 0.49 |
| (b) one-line edit → `cargo check`, runs 2–3 (run 1 warms the check cache: 5.8 / 2.3 s) | 1.5 / 1.3 s | 0.6 / 0.5 s | 0.39 |
| (c) clean build (deps + 50 views), 3 runs | 16.7 / 16.6 / 15.8 s | 5.9 / 6.3 / 6.1 s | 0.37 |
| (c) crates compiled / in the graph | 77 / 78 | 6 / 6 | — |
| (c) crates air adds to aio-rs | — | 2 (`air`, `air-macro`; proc-macro2 and quote are in already) | — |
| (d) held ↓, 1000 rows, keydown → applied, p50 (3 runs) | 1.3 / 1.3 / 1.3 ms | 0.7 / 0.7 / 0.7 ms | 0.54 |
| (d) the same, p95 | 1.6 / 1.6 / 1.7 ms | 0.8 / 1.1 / 1.0 ms | 0.6 |
| (d) bytes to the page per key | 66 | 48 | 0.73 |
| (d) held ↓, 10 000 rows, p50 (3 runs) | 5.2 / 5.2 / 5.3 ms | 1.0 / 1.0 / 2.5 ms | 0.19 |
| (e) hot patch | not measured (needs `dx`'s patch build wired to air) | remount: 1.4 ms (1000 rows) | — |

### Again on phase 1's air (4-core pin)

`bench.sh` run again on this branch's air (views built where placed,
component holes, typed events), the same machine pinned to 4 of its 32
threads (`taskset -c 0-3`, `CARGO_BUILD_JOBS=4`) with other sessions'
load on those cores (load average 13 over the run), so timings are
looser than above; 3 runs each, ratios of the medians.

| measure | Dioxus 0.7.10 | air (phase 1) | air / Dioxus | spike's ratio |
|---|---|---|---|---|
| (a) `cargo expand`, 1 view / 50 views | 385 KB / 18.9 MB | 89 KB / 4.4 MB | 0.23 | 0.17 |
| (a) crate metadata, 50 views | 6.18 MB | 1.94 MB | 0.31 | 0.28 |
| (a) LLVM IR lines (debug), 50 views | 3.65 M | 1.78 M | 0.49 | 0.32 |
| (a) debug binary, 50 views | 53.2 MB | 28.0 MB | 0.53 | 0.35 |
| (b) one-line edit → `cargo build`, 3 runs | 3.2 / 2.4 / 2.6 s | 1.4 / 1.4 / 1.3 s | 0.54 | 0.49 |
| (b) one-line edit → `cargo check`, runs 2–3 | 1.5 / 1.7 s | 0.6 / 0.5 s | 0.34 | 0.39 |
| (c) clean build, 3 runs | 33.8 / 30.8 / 32.7 s | 10.4 / 9.7 / 9.0 s | 0.30 | 0.37 |
| (c) crates compiled / in the graph | 77 / 78 | 8 / 8 | — | — |
| (d) held ↓, 1000 rows, p50 (3 runs) | 1.3 / 1.4 / 2.2 ms | 1.1 / 1.4 / 1.2 ms | 0.86 | 0.54 |
| (d) the same, p95 | 2.3 / 2.6 / 5.0 ms | 2.8 / 3.7 / 2.2 ms | — | 0.6 |
| (d) held ↓, 10 000 rows, p50 (3 runs) | 5.9 / 5.6 / 5.1 ms | 1.4 / 2.1 / 1.8 ms | 0.32 | 0.19 |
| (d) bytes to the page per key | 66 | 48 | 0.73 | 0.73 |
| (e) remount (first mount), 1000 / 10 000 rows | — | 1.6–3.4 / 13.7–25.6 ms | — | — |

Phase 1 gave back part of the spike's lead in code size: each view now
clones its captured names into a tree built later, and each component is
a hole whose closure builds its props (both needed for views built where
placed and for bodies that run again after a suspend). It stays at half
of Dioxus's IR and a third of its build times; the held key keeps its
shape (flat in the rows, 48 B), the p50 at 1000 rows within the load's
noise of Dioxus's here. Phase 2a found the IR's cause and took it back
(below: 0.31x).

The first prototype (generic holes), for the record: 50 views made
13.4 M lines of LLVM IR and a 168 MB debug binary; a one-line edit took
21.6 s (1 run).

How (d) was measured: `hold dx|air 200 40 <rows>` (release) runs the
same aio cell (`List { rows, cursor }`, `down()` per key), drawn either by
Dioxus liveview as aio-rs 0.1.0 draws it (`client!`, `part`, rows as
components with a `selected` prop, all rows drawn, keyed) or by air
(`air_client!`, a `selector` on the cursor), in the pinned Electron
44.9.3 runtime, headless, over the shell's own framing. ↓ goes in
through the DevTools protocol every 40 ms (a key repeat); the page's
preload times each keydown and each frame once applied
(`performance.now()`, 0.1 ms resolution). Most of air's 0.7 ms is the IPC
round trip. Dioxus's extra is re-rendering the list component and
diffing 1000 rows' props each step, which grows with the rows (5.2 ms at
10 000); air's work per key doesn't depend on the list's length. Run 3
of air at 10 000 rows met other load on the machine (p50 2.5 ms, against
1.0 ms twice).

Caveat for (a)–(c): generated views, not a real app. By the per-view
ratio, the 74 000-line app's 60 MB expansion would be about 10 MB.

## Phase 1: what it delivered

Feature `air` of aio-rs (`air` alone: 50 crates in the graph, no
`dioxus`, against `kit`'s 116; `ci.sh` fails on a planted `dioxus` there):

- **Core** (`crates/air`): signals, memos, effects (first run once the
  view is drawn), `watch`, `once`, `on_cleanup`, `batch`, `untrack`,
  `selector`; owners with context (`provide`, `provide_root`, `context`,
  `try_context`); callbacks (`Callback`, `EventHandler`, `callback`);
  `Global`, `GlobalMemo` per app instance; tasks (`spawn`, `spawn_forever`,
  `task`) on an executor of the thread's own, woken by the host;
  `resource` (`get`, `pending`, `restart`, `suspend`); `ErrorBoundary`,
  `Suspense`, `Fragment`; keyed lists; typed, bubbling events;
  `onmounted`; page scripts (`eval`); `Root` (one per thread) and
  `air::live()` (what is alive: the leak gate's measure).
- **The facade on air** (`crates/aio/src/ui/on_air.rs`): the
  `aio::ui::prelude` names, `eval` typed with serde; `client!`, `act!`
  and `use_css!` unchanged (they are written against the facade).
- **`TestUi` on air**: the Electron page with air's interpreter
  (`aio::page::body` on air), a task carrying the page's frames to the
  root and its edits back; and the Rust page (`air::testing::TestPage`)
  for air's own tests.
- **Gates 1–3**, each shown failing first (below).

Tests on air (Linux, `cargo test`): `aio-rs-air` 42 (lib 9, core 12,
gates 10, fault 3, views 5, doc 3); aio-rs on air: `air_ui` 5 of 5,
`facade` 7 of 10, `ui` 15 of 22, the lib's page, ui and testing tests 26
of 26. Not passing on air, by design (each `ignore`d on air with its
reason): the facade's and `ui`'s tests that count body re-runs or read
state in a body (a snapshot on air: 3 + 7). Not built on air: `kit.rs`
(67: `aio::kit` is Dioxus code until step 2). Since phase 2b the router
and every `TestUi` feature run on air too ([Phase 2b](#phase-2b-the-router-and-testui-on-air)).

Left for step 1: portals and `Defer` (kit components today: step 2 moves
them), `ax test` of the template app on air (the template is the kit's),
optional props. Left after it: the kit (step 2), `ax dev`'s hot reload
and the web and Android pages (step 3), the CI gates on the packaged app
(step 4).

## Phase 2b: the router and TestUi on air

- **The tree of mounted components** (`air::components()`, aio DevTools'
  `tree`, `crates/air/src/tree.rs`): each component an `rsx!` places, by
  name and key (its `Display`, else its `Debug`), with its body runs and
  its updates (a re-run of one of its own holes: what a change cost it).
  A hole knows its component from where it was made (a thread-local
  current component, set while a component's view is built and while a
  hole runs), so a count is an increment and a lookup by index; the tree
  is walked only when asked. A component's entry ends with its hole; a
  static component's hole is kept (it read nothing) so it stays in the
  tree.
- **`follow`**: in a body, reads that run the body again when they change
  (AIR's re-render, opt in), its hooks handing back what they made and
  its props built again. The facade's `follow(f)` is a memo on air (the
  body runs again only for a new value) and `f()` on Dioxus. The
  router's `Route` follows its match: a new place that keeps the match (a
  query, another route's place) runs nothing; a new param makes its
  element anew, so a body's `params()` is current. `provide_now` gives
  context per run. A `Link`'s active class and `aria-current` are holes.
- **`TestUi`**: `components()` from that tree; the page's interpreter
  names its nodes for the page's readers (`window.__airNode`), so a path
  can name a component by its first and last node (`comp=Row[42]`),
  `find(component, key)` returns that path, `within(t, f)` scopes a
  test's paths, and `surface_tree()` gives aio's `UISurfaceNode` shape
  (each component's elements and text). `advance(d)` pauses the views'
  tokio clock and sleeps `d` on it: a paused clock jumps to the next timer
  when the views idle, so each timer fires at its moment. `a11y()` runs
  aio's `dev-a11y.ts` rules on the page (the handler rules from the
  page's listener table).

## The app surface: `aio::ui`

What aio-rs apps named of Dioxus in 0.1.0 (templates, docs, the kit's public
API, `TestUi`; audit of the 0.1.0 tree):

| forced by aio's own signatures | used directly by apps |
|---|---|
| `Element` (kit `children`/slots, `TestUi::new`, `aio_shell::run`, `web::launch`, `Column::view`) | `rsx!`, `#[component]`, `Props` |
| `EventHandler<T>`, `Callback<A, R>` (every kit `on*` prop, `VirtualList.row`) | `use_signal`, `use_hook`, `use_effect`, `use_memo`, `use_resource` |
| `KeyboardEvent`, `MouseEvent`, `MountedEvent` (kit props, `use_global_key`) | `use_context` / `provide_context` |
| `Key`, `Modifiers` (`TestUi::press_key`, `ChordSpec::hit`) | `spawn`, `document::eval` |
| `GlobalSignal` (`client!`'s `HANDLE`, `TestUi::read`) | `FormEvent`, `onmounted`/`MountedData` methods |
| `ComponentFunction` (`TestUi::with`), `Attribute` (spread attributes) | `dangerous_inner_html` |

Step 0 (done, 0.2.0): one renderer-neutral surface, `aio::ui::prelude`, on
Dioxus 0.7 for now. The template, the kit, `ax ui`, the views `ax` writes,
the tests and the guide use it; the app template depends on `aio` alone
(feature `kit`); `aio` no longer re-exports `dioxus`. `ax lint` refuses a
`dioxus` path, a `dioxus` dependency and a page script's `dioxus.send`
(area `renderer`), and `ci.sh` greps the repo's own app-facing code for it.
Swapping the renderer is then a framework change, not an app migration.

### The facade's API

Each name already has the shape air provides; on Dioxus most are thin
adapters (`crates/aio/src/ui/view.rs`).

| area | names |
|---|---|
| views | `rsx!`, `#[component]`, `Props`, `View` (a `Result`: `?` throws to an `ErrorBoundary`), `ViewError`, `Component` (what `TestUi::with` mounts), `Fragment`, `Attribute` (`..attributes`, `#[props(extends = GlobalAttributes)]`) |
| handlers | `EventHandler<T>`, `Callback<A, R>`, `callback(f)` (made once) |
| local state | `signal(\|\| v) -> Signal<T>`; `Signal::new(v)` outside a view's body |
| `Signal<T>` (`Copy`) | `get`, `get_untracked`, `with`, `with_untracked`, `read`, `read_untracked` (guards), `set` (tells only on a change, `T: PartialEq`), `update(\|v\| ..)`, `write` (guard, tells on drop), `try_write`, `try_read_untracked`, `alive`, `toggle`, `+=`, `-=`; `Display`/`Debug` subscribe (`"{n}"`); as an attribute value it follows |
| derived | `memo(f) -> Memo<T>` (`get`, `with`, `read`, …), `watch(deps, f)` (an effect when values the view made change) |
| effects, lifetime | `effect(f)`, `once(\|\| v)` (made once per view), `on_cleanup(f)` |
| app-wide | `Global<T>` (`Global::new(\|\| v)` in a `static`; the `Signal` methods), `GlobalMemo<T>` |
| context | `provide(\|\| v)`, `provide_root(v)`, `context::<T>()`, `try_context::<T>()` |
| async | `spawn`, `spawn_forever`, `Task`, `task(\|\| fut)` (the view's life), `resource(\|\| fut) -> Resource<T>` (`get`, `with`, `pending`, `restart`, `suspend`) |
| boundaries | `ErrorBoundary { fallback: \|e: String\| rsx! {..}, .. }`, `Suspense { fallback: rsx! {..}, .. }` |
| events | `Event<T>` and its kinds (`KeyboardEvent`, `MouseEvent`, `FormEvent`, `WheelEvent`, `ScrollEvent`, `FocusEvent`, `PointerEvent`, `DragEvent`, `MountedEvent`, …) with their methods (`key()`, `modifiers()`, `value()`, `prevent_default()`, …), `Key`, `Code`, `Location`, `Modifiers`, `MouseButton`, `geometry`, `key_event(key, mods, repeat)` |
| page | `eval(js) -> Eval` (`send`, `recv`, `join`; the script's page object is `aio`: `aio.send(v)`, `await aio.recv()`), `in_views()` |
| cells (`client!`) | `part(\|c\| &c.f)` (a handle: `get`, `with`, `ver`, `"{p}"`), the whole-state reader, `<read>_at(f) -> Memo`, `use_live`, `use_status`, `use_pending`, `use_time_travel`, `act!` |
| routing (`router!`) | `here`, `go`, `redirect`, `back`, `use_back`, `Route`, `Outlet`, `Link`, `Redirect`, `params`; `routes!` |
| styles | `use_css!` |
| tests | `TestUi::new(fn() -> View)`, `TestUi::with(C, props)`, `ui.read(&GLOBAL)`, `Key`, `Modifiers` |

The functions that keep something for a view (`signal`, `memo`, `effect`,
`watch`, `once`, `on_cleanup`, `callback`, `provide`, `task`, `resource`)
are called at a view's top level, in the same order each time: Dioxus's hook
rule, which `ax lint` holds them to as it holds `use_…` hooks; air runs a
view once, so the same code is right there. The kit's own hooks keep their
names (`use_form`, `use_global_key`, `use_spring`, …): plain functions in air.

Reads take air's shape: a `Signal` is not callable (`count()` is
`count.get()`, Rust can't make a handle callable on stable), `peek` is
`read_untracked`/`get_untracked`, `with_mut` is `update`, and a part is a
handle (`*part(…)` was a value; now `part(…).get()`). `set` already
compares (aio's AIR `Object.is`), so setting what is there renders nothing,
as on air.

### What can't match air yet, and why

- **A view's body runs on every render** (Dioxus), **once** on air. Code
  that reads state in the body (`let w = wallet();`, then `w.x` in `rsx!`)
  follows it on Dioxus and is a snapshot on air. No facade can change when
  a body runs: the reads move into the holes (`"{n}"`, `n.get()` in an
  attribute) or into a `memo` when the swap comes (step 2; a lint spots the
  body reads it can).
- **Props are evaluated once on air.** On Dioxus a parent's re-render
  hands new props. `watch(deps, f)` is the facade's bridge for effects that
  follow values a view made from props (0.1's `use_effect(use_reactive!(…))`);
  on air a prop that must follow state is a `Signal`, a `Part` or a
  `Prop<T>` (the kit's props are `Prop`s: `value: name.get()` follows).
- **Events**: air has its own payload types with Dioxus's names and
  methods (`KeyboardEvent`, `e.key()` a `keyboard-types` `Key`,
  `e.modifiers()`, `e.value()`, `e.client_coordinates()`, …), so handler
  code is the same; `prevent_default()` can't reach the page in time on
  either (the page keeps keys with `data-aio-prevent`; a handled `submit`
  and a handled link click never navigate). Not decoded yet: files, a
  drag's data, touch lists (step 2, with the kit's file inputs).
- **`MountedData`** on air: `set_focus`, `scroll_to`, `scroll`,
  `get_client_rect`, and `run` / `eval` (a script with the element as
  `el`: a canvas, a video, a selection), as page calls on the element
  (`eval` with `aio.node(id)`).
- **`key:` and `if let`**: air makes a row without `key:` and `if let` in
  `rsx!` compile errors, and two rows with one key a failure in every
  build. On Dioxus `ax lint` makes the missing key an error and warns on
  `if let`.
- **`mut` on a signal**: Dioxus needs `let mut n = signal(…)` for a handler
  that writes `n`; on air the handler holds its own copy, so `mut` is
  unused (a warning).
- **On air only** (phase 2a, aio's): labels as views, `form.bind`,
  `reset_field`, `use_field_array`, `use_on_change`, `Defer`'s `load`,
  `Transition` without a box, `Motion::of`, `aio::ui::css`. The kit on
  Dioxus keeps 0.1's text labels and boxes until step 5 removes it.
- **Hot patch**: Dioxus keeps a view's local state across a patch; air
  remounts (cells keep theirs).
- **What stays on Dioxus inside the framework** until step 5: the kit,
  `web::launch`, the server's first paint, the shell's liveview socket and
  `ax dev`'s `dx`. With feature `air`, the page (`aio::page::body`) and
  `TestUi` are air's.

## Gates before Dioxus goes (each failing first)

1. **No subscription leaks** (green, `crates/air/tests/gates.rs`): a
   component removed, or replaced by an error boundary in the same pass,
   leaves no node, effect, subscription, cleanup, handler, task or page
   script (`air::live()`) and no named page node; a dropped root leaves
   nothing at all. Shown red first by three planted leaks (rows that
   outlive their list, handlers never removed, hooks' state that outlives
   its view).
2. **Misuse fails in release too** (green, the same file, run with
   `--release` by `ci.sh`): a hook outside a view's body (or in a
   handler), a write while a hole or a memo computes, an effect loop (the
   round guard, 1000 rounds as AIR's), a signal used after its view, a
   hooks in another order on a re-run, two rows with one
   key; a `for` without `key:` and `if let` are
   compile errors (`compile_fail` doc tests). Shown red first by making
   the write check and the round guard debug-only and running `--release`
   (the write went through; the loop never ended).
3. **TestUi catches a planted break** (green): feature `fault` of air
   plants an unbound text hole or a lost list row; the same test that
   passes on the sound renderer fails, in the Rust page
   (`crates/air/tests/fault.rs`) and in `TestUi`'s Electron page
   (`crates/aio/tests/air_ui.rs`).
4. **No Dioxus in the package**: a CI check that the packaged binary's
   dependency graph (`cargo tree -e normal` of the release target) and its
   symbols contain no `dioxus`.
5. **Measured on the packaged app**: size, clean build and edit→rebuild
   of one real app on AppImage / exe / APK, by a script in the repo
   (this spike's `bench.sh` is the generated-app half).

## Release rule and scope

- One renderer per release: no flag, no dual path in a release build.
  Dioxus may live beside air on the dev branch only. If air isn't through
  the gates, 0.2.0 ships on Dioxus and air moves to the next minor.
- Out of scope, intentional cuts (listed in `docs/aio-vs-aio-rs.md`):
  server rendering, hydration, head collection, code splitting.

## What Dioxus covers today that air must replace

| covered by Dioxus | air's replacement | effort |
|---|---|---|
| `rsx!` template hot reload (dx devtools) | `air-rsx` parser in `ax dev`, template override by location, remount | 1 week |
| subsecond hot-patch of views | `register_handler` → remount `HotFn::current(app)`; `dx` still builds the patch | 3 days |
| web target (`dioxus-web`) | `page.js` in-process, `__airApply` on wasm memory, events to a wasm export | 1 week |
| Android | the same page body over the existing bridge | 2 days |
| the kit (22 modules, 5.4 k lines) | port to the facade's names, then to air: slots as `View`, handlers as `FnMut`, `onmounted` as page calls | 3–4 weeks |
| `TestUi` (1.4 k lines on the VirtualDom) | the Rust page (prototype) + the Electron page (`page.js` instead of Dioxus's glue), same queries | 1–2 weeks |
| liveview interpreter, events, focus, file inputs | `page.js` + aio's page scripts re-pointed (focus, typing, keys, file inputs patch Dioxus's interpreter today) | 1 week |

## Phase 2a: the kit on air

- Every kit module runs on air (`crates/aio/src/kit_air`, the kit's
  `#[path]` twin while Dioxus is the default): a body runs once, so each
  value the kit read in its body is a `memo` or a hole, and each prop that
  follows its caller is a `Prop`. The kit, facade, ui and sign-in tests
  run on air in CI (`ci.sh`'s air lane), none ignored: the tests that
  counted body re-runs count a hole's runs, which holds both renderers to
  AIR's rule (what didn't read a change doesn't re-run).
- Roots on one thread are apart: each node records its root, effects,
  tasks, handlers and deferred calls run in it, and each `Root` has its
  own page state, edits and globals (`Root::take_edits`).
- Theming: `[ui] scheme` (the OS's light or dark as it changes, or one
  fixed), `use_dark()`, `[ui] neutral`.
- Gap rows VW2–VW15 (aio-rs-internal's gaps-0.2 list) are done on air;
  VW5 (`Intl`) and VW16 (Standard Schema) are not air's.

### Measured on phase 2a's air

The 0.49x IR of phase 1 was the handler: its wrappers were generic over
the closure, so each `onclick` made its own `Rc<RefCell<…>>`, drop glue
and shims, 36% of the 50-view crate's IR. `rsx!` boxes the closure and
the wrappers are generic over the payload alone: 1.81 M lines to 1.13 M,
under the spike's 0.32x. `scripts/air-ir` keeps it.

`bench.sh` again, on this branch: the same machine pinned to 12 of its
32 threads (`taskset -c 0-11`, `CARGO_BUILD_JOBS=6`) with other
sessions' load (load average 16–20 over the run); 3 runs each, ratios of
the medians.

| measure | Dioxus 0.7.10 | air (phase 2a) | air / Dioxus | phase 1 | spike |
|---|---|---|---|---|---|
| (a) `cargo expand`, 1 view / 50 views | 385 KB / 18.9 MB | 96 KB / 4.76 MB | 0.25 | 0.23 | 0.17 |
| (a) crate metadata, 50 views | 6.18 MB | 2.68 MB | 0.43 | 0.31 | 0.28 |
| (a) LLVM IR lines (debug), 50 views | 3.65 M | 1.13 M | 0.31 | 0.49 | 0.32 |
| (a) debug binary, 50 views | 53.2 MB | 17.3 MB | 0.32 | 0.53 | 0.35 |
| (b) one-line edit → `cargo build`, 3 runs | 2.5 / 2.5 / 2.4 s | 1.2 / 1.1 / 1.1 s | 0.44 | 0.54 | 0.49 |
| (b) one-line edit → `cargo check`, runs 2–3 | 1.4 / 1.4 s | 0.9 / 0.7 s | 0.57 | 0.34 | 0.39 |
| (c) clean build, 3 runs | 25.0 / 23.7 / 23.8 s | 7.3 / 7.3 / 7.0 s | 0.31 | 0.30 | 0.37 |
| (c) crates compiled / in the graph | 77 / 78 | 8 / 8 | — | — | — |
| (d) held ↓, 1000 rows, p50 (3 runs) | 1.3 / 1.3 / 1.6 ms | 0.8 / 0.7 / 0.7 ms | 0.54 | 0.86 | 0.54 |
| (d) the same, p95 | 1.8 / 1.9 / 2.5 ms | 1.2 / 1.2 / 1.3 ms | 0.63 | — | 0.6 |
| (d) held ↓, 10 000 rows, p50 (3 runs) | 5.3 / 5.3 / 5.8 ms | 1.1 / 1.1 / 1.0 ms | 0.21 | 0.32 | 0.19 |
| (d) bytes to the page per key | 66 | 48 | 0.73 | 0.73 | 0.73 |
| (e) first mount, 1000 / 10 000 rows | — | 1.4–2.0 / 11.1–15.4 ms | — | 1.6–3.4 / 13.7–25.6 | 1.4 / 8.3 |

The metadata grew with the props traits the kit needs (a view prop takes
a view, a text or a thunk: `IntoView`, `IntoViewValue`), still 0.43x;
`cargo check`'s 0.7–0.9 s is within this load's noise of phase 1's
0.5–0.6 s. The first mount: of 1.4–2.0 ms at 1000 rows, 0.6–0.7 ms fills
the aio cell; at 10 000 rows the fill is 1.6–1.7 ms, the rest about 1 µs a
row for the views (the spike drew fixed rows without a cell).

## Plan for 0.2.0

In this order; each step ends green before the next starts. Dioxus stays
on the dev branch as the fallback until step 5.

| # | step | what | done when | estimate |
|---|---|---|---|---|
| 0 | facade (done) | `aio::ui::prelude` on Dioxus; template, docs, kit, `ax ui`, `TestUi`, `aio_shell::run`, `web::launch` take the facade's names; the template's `dioxus` dependency gone | no `dioxus` in the template, the kit, `ax ui`, the tests or the guide (`ci.sh`, `ax lint`); 0.1.0 apps move with the migration guide's table | 1 week |
| 1 | air core (done but portals, `Defer`, the template app) | from the spike: holes, lists (live row ranges), branches, context over owners, error boundary, refs and page calls (`onmounted`), portals, `Defer`; `TestUi` on the Rust page and the Electron page; gates 1–3 written failing, then green | `cargo test -p aio-rs-air`, the gate tests, `ax test` of the template app on air | 2–3 weeks |
| 2 | the kit on air (2a done: the kit, theming, forms; 2b: the router) | the 22 kit modules behind the facade, on air (dev branch: the facade's renderer chosen by a dev-branch feature, never in a release) | the kit's tests and docs examples on air | 3–4 weeks |
| 3 | tooling | `ax dev`: template hot reload (`air-rsx`), subsecond remount; web (wasm, in-process `page.js`); Android page body | `ax dev` edit loops, `ax build --web`, an APK smoke | 2–3 weeks |
| 4 | gates in CI | no Dioxus in the packaged graph or binary; size and build/edit numbers of the packaged app by a script in the repo | CI fails on a planted Dioxus dependency | 3 days |
| 5 | delete Dioxus | out of the default graph and `deps.toml`; one real app rebuilt and smoked on Linux, Windows, macOS | the smoke on three OSes; the numbers in `docs/performance.md` | 1 week |

About 10–13 weeks of work. How apps migrate: step 0 is the one API change
(docs/migration, "0.1 → 0.2": `use aio::ui::prelude::*`, the facade's
names, `count.get()`, `part(…).get()`, `key:` on rows); from step 2 on, the
remaining edits are the ones under *What can't match air yet* (body reads
into holes, reactive props as signals), with an `ax lint` rule for each
that it can spot.

If the gates aren't green when 0.2.0 freezes, 0.2.0 ships on Dioxus with
the facade (steps 0 and 4's size script), and air moves to 0.3.0. One
renderer per release either way.

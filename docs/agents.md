# aio-rs for coding agents

`ax agent` is the brief an agent reads before it touches an aio-rs app:
the rules that protect the person first, then how aio-rs works, then every
verb. It is made from the code (the verb table, the brief's sections), so
it matches the `ax` installed.

```sh
ax agent              # the page
ax agent --min        # compact forms
ax agent --max        # with the deep sections (api, cli, pitfalls)
ax agent --list       # the sections
ax agent ui           # one section in full
ax agent --json       # the same, addressable
```

The sections: `rules`, `model`, `test`, `debug`, `ship`, `new`, `cell`,
`ui`, `sync`, `data`, `api`, `cli`, `pitfalls` (`ax agent --list` says
which are deep, shown only with `--max` or by name).

Every app `ax create` makes has an `AGENTS.md` (and a `CLAUDE.md` that
imports it): the rules and the loop below, inline, then the brief and this
guide by URL (no path of the machine it was made on).

## The rules

1. Never kill by process match (`pkill`, `killall`): `ax stop <app>`.
2. Never take over the person's screen: an agent's windows go on a nested
   display (`ax dev` does it by itself when it has no terminal);
   `ax build --smoke` and UI tests run headless.
3. Never script around `ax`: assert with `ax expect`, watch with
   `ax state --watch`, read `--json`.
4. Learn before editing: read the brief, then the code around.
5. Check, run, test: `cargo check`, `ax dev <app>`, tests that prove it.
6. Clean up only what you made: a throwaway app runs with
   `AIORT_DATA=<dir>` for every verb; done, `ax stop` and delete that dir.

## The loop

| Step | Verbs |
|---|---|
| understand | `ax agent`, `cargo doc -p aio-rs --no-deps` (read the files, never `--open`) |
| run | `ax dev <app>`, `ax start <app>` |
| look | `ax state`, `ax events`, `ax surface`, `ax shot`, `ax logs` |
| act | `ax call`, `ax trigger` |
| prove | `ax expect`, `ax test <app>` |
| debug | [debugging](debugging/README.md) |

Porting an aio app: [migration](migration/README.md), keeping aio's
optimizations ([performance](performance.md)).

## A throwaway run

```sh
AIORT_DATA=/tmp/try-notes ax start notes --fresh
AIORT_DATA=/tmp/try-notes ax call notes counter add 2
AIORT_DATA=/tmp/try-notes ax expect notes counter count eq 2
AIORT_DATA=/tmp/try-notes ax stop notes
rm -rf /tmp/try-notes
```

The same `AIORT_DATA` on every verb reaches the same copy; its data, logs
and cache stay in that folder (only its sockets live under
`$XDG_RUNTIME_DIR/aio-rt`, else the temp dir).

## The brief's view examples

Every example `ax agent ui` shows in backticks is in a block below: the
first compiles and runs (a doctest of aio-rs), each after it fails to
compile, as the brief's "NO" says. A test of `ax` checks that each of the
brief's examples is here, so the brief can't show code that doesn't build.

```rust
use aio::ui::prelude::*;

#[aio::data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub id: u32,
    pub title: String,
}

#[aio::cell(id = "todo")]
pub struct Todo {
    pub items: Vec<Item>,
    pub title: String,
}

#[aio::methods]
impl Todo {
    pub fn add(&mut self, title: String) {
        self.items.push(Item { id: self.items.len() as u32, title });
    }
}

aio::client!(Todo, todo);

#[derive(Clone, PartialEq)]
pub enum Page {
    Home,
    Item(u32),
}

/// The last run's sum: shown by every view, never saved.
static REPORT: Global<Option<u64>> = Global::new(|| None);

fn sum(n: u64) -> u64 {
    (1..=n).sum()
}

mod page {
    use super::{Item, Page, REPORT, sum};
    use crate::prelude::*;

    #[component]
    pub fn App() -> View {
        use_live();
        let on = signal(|| true);
        let note = signal(|| Some("a note".to_string()));
        let page = signal(|| Page::Item(7));
        let items = part(|t| &t.items);
        rsx! {
            button { "data-t": "add",
                onclick: move |_| {
                    let title = todo().title.clone() + "milk";
                    _ = act!(add(title));
                },
                "Add"
            }
            button { "data-t": "run",
                onclick: move |_| _ = spawn_forever(async move { REPORT.set(aio::effect::off_thread("sum", sum, 1000).await.ok()); }),
                "Run"
            }
            div { "data-t": "on", if on.get() { p { "on" } } else { p { "off" } } }
            div { "data-t": "note", if let Some(n) = note.get() { p { "{n}" } } }
            div { "data-t": "page", match page.get() { Page::Home => p { "home" }, Page::Item(id) => p { "{id}" } } }
            ul { "data-t": "rows", for t in items.get() { li { key: "{t.id}", "{t.title}" } } }
            ul { "data-t": "rows-by-component", for t in items.get() { Row { key: "{t.id}", item: t } } }
            p { "data-t": "report", "{REPORT.get().unwrap_or_default()}" }
        }
    }

    #[component]
    fn Row(item: Item) -> View {
        rsx! { li { "{item.id}: {item.title}" } }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(page::App);
assert_eq!((ui.text("on"), ui.text("note"), ui.text("page")), ("on".into(), "a note".into(), "7".into()));
ui.click("add");
assert_eq!((ui.text("rows"), ui.text("rows-by-component")), ("milk".into(), "0: milk".into()));
ui.click("run");
ui.until_text("report", "500500");
assert_eq!(ui.read(&REPORT), Some(500500));
# }
```

A `for` row with no `key:`:

```rust,compile_fail
use aio::ui::prelude::*;
fn rows() -> View {
    rsx! { ul { for n in 0..3 { li { "{n}" } } } }
}
```

A `while` (or a `loop`):

```rust,compile_fail
use aio::ui::prelude::*;
fn count(mut n: u32) -> View {
    rsx! { while n > 0 { p { "{n}" } } }
}
```

A row that is a block: the key inside it keys nothing.

```rust,compile_fail
use aio::ui::prelude::*;
#[derive(Clone, PartialEq)]
struct Item {
    id: u32,
}
fn rows(items: Signal<Vec<Item>>) -> View {
    rsx! { ul { for t in items.get() { { let id = t.id; rsx! { li { key: "{id}" } } } } } }
}
```

A field that isn't `Copy`, moved out of the state (an `Arc`): clone it.

```rust,compile_fail,E0507
#[aio::cell(id = "todo")]
pub struct Todo {
    pub title: String,
}

#[aio::methods]
impl Todo {}

aio::client!(Todo, todo);

fn title() -> String {
    let title = todo().title;
    title
}
# fn main() {}
```

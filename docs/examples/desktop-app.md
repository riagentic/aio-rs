# A desktop app

A local-first notes app for the desktop: notes saved on every keystroke,
pages with links and Back, a menu, keyboard shortcuts, files exported and
imported through the system's dialogs, a tray icon, a single instance
that opens files handed to it, and updates from signed releases. No
server, no account: the notes live in the app's own data on this
machine.

You will use:

- a `persist, coalesce` cell, and effects for file I/O;
- `aio::routes!` and `aio::router!` for the pages;
- the kit's `Menu`, `GlobalKeys`, `ToastHost`;
- `aio_shell` for the window: open and save dialogs, the tray, later
  launches, the `updates` cell;
- `aio::testing::TestCells` and `TestUi` for its tests.

Every Rust block on this page compiles as a test of aio-rs's desktop
host (`aio-rs-shell`, whose own API it calls), and the cell's logic runs.
Prerequisites: [the tutorial](../basics/tutorial.md),
[views](../ui/views.md) and [routing](../ui/routing.md).

Start from an app (`ax create notes`) and put the code below in
`src/lib.rs`. The library calls `aio_shell` for the dialogs and the
tray, so it depends on it too: in the app's `Cargo.toml`,
`aio-shell = { package = "aio-rs-shell", path = "<the aio-rs checkout>/crates/aio-shell" }`
beside `aio`.

## 1. The cell

```rust
use aio::{Cx, Effect, Handle, Uuid, ensure, reject};
use std::path::Path;
use std::sync::Arc;

#[aio::data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Note {
    pub id: Uuid,
    pub title: String,
    pub body: String,
}

/// Every note, kept on each keystroke: shown at once, written within 100 ms.
#[aio::cell(persist, coalesce, id = "notes")]
pub struct Notes {
    pub notes: Vec<Note>,
    pub search: String,
    /// The last file problem, for the window to say.
    pub problem: Option<String>,
}

/// The largest file `import` reads.
pub const MAX_IMPORT: u64 = 1 << 20;

/// Off the async threads: file I/O blocks.
async fn blocking<R: Send + 'static>(f: impl FnOnce() -> std::io::Result<R> + Send + 'static) -> Result<R, String> {
    match aio::rt().spawn_blocking(f).await {
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

fn read_note(path: &str) -> std::io::Result<String> {
    let size = std::fs::metadata(path)?.len();
    if size > MAX_IMPORT {
        return Err(std::io::Error::other(format!("{path} is {size} bytes; at most {MAX_IMPORT}")));
    }
    std::fs::read_to_string(path)
}

#[aio::methods]
impl Notes {
    pub fn create(&mut self, cx: &Cx, title: String, body: String) -> aio::Result<Uuid> {
        let id = cx.id();
        let title = if title.trim().is_empty() { "Untitled".to_string() } else { title };
        self.notes.insert(0, Note { id, title, body });
        Ok(id)
    }

    pub fn edit(&mut self, id: Uuid, body: String) -> aio::Result {
        let Some(n) = self.notes.iter_mut().find(|n| n.id == id) else { reject!("no note {id}") };
        n.body = body;
        Ok(())
    }

    pub fn rename(&mut self, id: Uuid, title: String) -> aio::Result {
        let Some(n) = self.notes.iter_mut().find(|n| n.id == id) else { reject!("no note {id}") };
        n.title = title;
        Ok(())
    }

    pub fn delete(&mut self, id: Uuid) {
        self.notes.retain(|n| n.id != id);
    }

    pub fn find(&mut self, query: String) {
        self.search = query;
    }

    /// The titles the sidebar lists, filtered; kept while notes and search stay.
    #[derived(reads = [notes, search])]
    pub fn shown(&self) -> Arc<Vec<(Uuid, String)>> {
        let q = self.search.to_lowercase();
        let hit = |n: &&Note| q.is_empty() || n.title.to_lowercase().contains(&q) || n.body.to_lowercase().contains(&q);
        Arc::new(self.notes.iter().filter(hit).map(|n| (n.id, n.title.clone())).collect())
    }

    /// Write note `id` to `path` (a save dialog's answer) as Markdown.
    pub fn export(&mut self, cx: &Cx, id: Uuid, path: String) -> aio::Result {
        let Some(n) = self.notes.iter().find(|n| n.id == id) else { reject!("no note {id}") };
        let text = format!("# {}\n\n{}\n", n.title, n.body);
        cx.run(Effect::new("export", move |h: Handle<Notes>| async move {
            let to = path.clone();
            let wrote = blocking(move || aio::paths::write_atomic(Path::new(&to), text.as_bytes(), false)).await;
            _ = h.exported(path, wrote);
        }));
        Ok(())
    }

    pub fn exported(&mut self, cx: &Cx, path: String, out: Result<(), String>) -> aio::Result {
        match out {
            Ok(()) => cx.tell(aio::tell::Tell::notify("Exported", &path)),
            Err(why) => {
                self.problem = Some(format!("couldn't export: {why}"));
                Ok(())
            }
        }
    }

    /// Read the file at `path` into a new note (an open dialog's answer, or
    /// a file the app was launched with).
    pub fn import(&mut self, cx: &Cx, path: String) -> aio::Result {
        ensure!(path.ends_with(".md") || path.ends_with(".txt"), "a .md or .txt file, please");
        cx.run(Effect::new("import", move |h: Handle<Notes>| async move {
            let from = path.clone();
            let read = blocking(move || read_note(&from)).await;
            _ = h.imported(path, read);
        }));
        Ok(())
    }

    pub fn imported(&mut self, cx: &Cx, path: String, out: Result<String, String>) -> aio::Result {
        match out {
            Ok(text) => {
                let name = Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                self.create(cx, name, text)?;
                self.problem = None;
            }
            Err(why) => self.problem = Some(format!("couldn't import: {why}")),
        }
        Ok(())
    }
}

# fn main() {
use aio::testing::TestCells;
let dir = tempfile::tempdir().unwrap();
let at = |f: &str| dir.path().join(f).to_string_lossy().into_owned();
let mut t = TestCells::new();
let h = t.cell::<Notes>();

let id = h.create("Groceries".into(), "milk".into()).unwrap();
h.edit(id, "milk, tea".into()).unwrap();
h.create("Ideas".into(), String::new()).unwrap();
h.find("tea".into()).unwrap();
assert_eq!(*h.state().shown(), [(id, "Groceries".to_string())], "found in the body");

let mut told = h.tells();
h.export(id, at("groceries.md")).unwrap();
t.settle(); // the effect writes the file, then `exported` commits
assert_eq!(std::fs::read_to_string(at("groceries.md")).unwrap(), "# Groceries\n\nmilk, tea\n");
assert!(matches!(told.try_recv(), Ok(aio::tell::Tell::Notify { title, .. }) if title == "Exported"));

h.import(at("groceries.md")).unwrap();
t.settle();
assert_eq!(h.state().notes[0].title, "groceries");
assert!(h.import(at("photo.png")).is_err(), "refused before any I/O");
h.import(at("missing.md")).unwrap();
t.settle();
assert!(h.state().problem.as_deref().unwrap().starts_with("couldn't import"));
# }
```

- **`coalesce`**: every plain method shows its state at once and is
  written within 100 ms, so typing costs one disk write per pause, not
  per key. The history still holds each call (`ax events notes`).
- **Files are effects.** `export` and `import` decide (which note, which
  kinds of file) and ask for the I/O; its outcome comes back as a call
  (`exported`, `imported`). A replay never touches the disk.
- **Atomic writes**: `aio::paths::write_atomic` writes a temp file,
  syncs and renames, so a crash leaves the old file or the new one.
- **`cx.tell`** shows a desktop notification once the call commits
  ([effects](../state/effects.md#telling-people)).
- **Limits on input**: `import` refuses other file kinds before any I/O,
  and `read_note` refuses a file over 1 MB.

## 2. The window

```rust,no_run
# use aio::{Cx, Uuid};
# use std::sync::Arc;
# #[aio::data]
# #[derive(Clone, Debug, Default, PartialEq)]
# pub struct Note { pub id: Uuid, pub title: String, pub body: String }
# #[aio::cell(persist, coalesce, id = "notes")]
# pub struct Notes { pub notes: Vec<Note>, pub search: String, pub problem: Option<String> }
# #[aio::methods]
# impl Notes {
#     pub fn create(&mut self, cx: &Cx, title: String, body: String) -> aio::Result<Uuid> {
#         let id = cx.id();
#         self.notes.insert(0, Note { id, title, body });
#         Ok(id)
#     }
#     pub fn edit(&mut self, id: Uuid, body: String) { if let Some(n) = self.notes.iter_mut().find(|n| n.id == id) { n.body = body } }
#     pub fn rename(&mut self, id: Uuid, title: String) { if let Some(n) = self.notes.iter_mut().find(|n| n.id == id) { n.title = title } }
#     pub fn delete(&mut self, id: Uuid) { self.notes.retain(|n| n.id != id); }
#     pub fn find(&mut self, query: String) { self.search = query; }
#     #[derived(reads = [notes, search])]
#     pub fn shown(&self) -> Arc<Vec<(Uuid, String)>> { Arc::new(self.notes.iter().map(|n| (n.id, n.title.clone())).collect()) }
#     pub fn export(&mut self, id: Uuid, path: String) {}
#     pub fn import(&mut self, path: String) {}
# }
use aio::act;
use aio::kit::{GlobalKeys, Menu, MenuItem, ToastHost, Tone, toast, use_global_key};
use aio_shell::page::{Pick, Save};
use aio::ui::prelude::*;

aio::client!(Notes, notes);
aio::router!();

aio::routes! {
    pub enum Page {
        Home = "/",
        Note { id: Uuid } = "/note/:id",
        Settings = "/settings" => "Settings",
    }
}

/// A new note, opened. Also the tray's "New note" and Ctrl+N (⌘N).
pub fn new_note() {
    if let Some(id) = act!(create("Untitled".into(), String::new())) {
        go(&Page::Note { id }.href());
    }
}

const MARKDOWN: fn() -> Vec<(String, Vec<String>)> = || vec![("Notes".into(), vec!["md".into(), "txt".into()])];

/// The system's save dialog, then the export. A cancel is no path.
async fn ask_export(id: Uuid, title: String) {
    let ask = Save { title: "Export note".into(), suggest: Some(format!("{title}.md")), filters: MARKDOWN() };
    match aio_shell::page::save(&ask).await {
        Ok(Some(path)) => _ = act!(export(id, path)),
        Ok(None) => {}
        Err(e) => _ = toast(format!("No save dialog: {e}"), Tone::Error),
    }
}

async fn ask_import() {
    let ask = Pick { title: "Import a note".into(), filters: MARKDOWN(), ..Pick::default() };
    match aio_shell::page::pick(&ask).await {
        Ok(Some(path)) => _ = act!(import(path)),
        Ok(None) => {}
        Err(e) => _ = toast(format!("No open dialog: {e}"), Tone::Error),
    }
}

#[component]
pub fn App() -> View {
    use_live(); // once, in the root
    updates::use_live();
    aio_shell::desktop::use_asks(go); // tray clicks: their `run`, then their route
    once(|| {
        // A later launch (`notes todo.md` while it runs) hands its files here.
        let mut later = aio_shell::launches();
        spawn(async move {
            while let Ok(l) = later.recv().await {
                for a in l.args.iter().filter(|a| a.ends_with(".md")) {
                    _ = act!(import(l.path(a)));
                }
            }
        })
    });
    let theme = aio::kit::theme_css(aio::kit::theme::hue("notes"));
    rsx! {
        style { {aio::ui::page_css(&[theme.as_str()], false)} }
        GlobalKeys {
            ToastHost {}
            updates::Banner {}
            div { class: "notes",
                Sidebar {}
                main { Body {} }
            }
        }
    }
}

#[component]
fn Sidebar() -> View {
    use_global_key("mod+n", |_| new_note()); // under GlobalKeys: bindings fire there
    let search = part(|n| &n.search);
    let shown = notes().shown(); // derived: computed once per notes and search
    rsx! {
        nav {
            button { "data-t": "new", onclick: |_| new_note(), "New note" }
            input { "data-t": "search", r#type: "search", placeholder: "Search", value: "{search}",
                oninput: move |e| _ = act!(find(e.value())) }
            for (id, title) in shown.iter() {
                Link { key: "{id}", to: Page::Note { id: *id }.href(), "{title}" }
            }
            Link { to: Page::Settings.href(), "Settings" }
        }
    }
}

#[component]
fn Body() -> View {
    match Page::parse(&here().path) {
        Some(Page::Note { id }) => rsx! { Editor { id } },
        Some(Page::Settings) => rsx! { p { "data-t": "settings", "Settings go here." } },
        _ => rsx! { p { "data-t": "empty", "Pick a note, or make one (Ctrl+N)." } },
    }
}

#[component]
fn Editor(id: Uuid) -> View {
    let all = part(|n| &n.notes);
    let Some(note) = all.with(|ns| ns.iter().find(|n| n.id == id).cloned()) else {
        return rsx! { p { "data-t": "gone", "This note is gone." } };
    };
    let title = note.title.clone();
    rsx! {
        header {
            input { "data-t": "title", value: "{note.title}", oninput: move |e| _ = act!(rename(id, e.value())) }
            Menu { label: "Note", trigger: rsx! { "⋯" },
                items: vec![
                    MenuItem::new("export", "Export…"),
                    MenuItem::new("import", "Import…"),
                    MenuItem::new("delete", "Delete"),
                ],
                onselect: move |what: String| match what.as_str() {
                    "export" => _ = spawn(ask_export(id, title.clone())),
                    "import" => _ = spawn(ask_import()),
                    _ => {
                        _ = act!(delete(id));
                        redirect("/");
                    }
                },
            }
        }
        textarea { "data-t": "body", value: "{note.body}", oninput: move |e| _ = act!(edit(id, e.value())) }
    }
}

/// The `updates` cell's own views: an offer, and the two answers to it.
pub mod updates {
    use aio::act;
    use aio::update::{Phase, Updates, UpdatesCalls};
    use aio::ui::prelude::*;

    aio::client!(Updates, updates);

    #[component]
    pub fn Banner() -> View {
        let phase = part(|u| &u.phase).get();
        let offer = part(|u| &u.available).get();
        let (Phase::Available, Some(o)) = (phase, offer) else { return rsx! {} };
        rsx! {
            div { role: "status", "data-t": "update",
                "Version {o.version} is ready. {o.notes} "
                button { onclick: |_| _ = act!(apply()), "Restart to update" }
                button { onclick: |_| _ = act!(dismiss()), "Not now" }
            }
        }
    }
}

# fn main() {
let mut ui = aio::testing::TestUi::new(App);
ui.click("new");
ui.type_text("body", "milk");
ui.press("body", "ctrl+n");
assert_eq!(HANDLE.read_untracked().state().notes.len(), 2);
# }
```

- **Pages**: `routes!` types the places, `router!` gives the window its
  history (`here()`, `go`, `redirect`, `Link`, Back). `redirect` after a
  delete leaves no Back step to a note that is gone.
- **The menu** is the kit's `Menu`: a button with a list, arrows and
  Escape, items by id. Shortcuts are `use_global_key` under `GlobalKeys`;
  `mod` is Ctrl, or ⌘ on macOS.
- **Dialogs**: `aio_shell::page::pick`, `pick_many` and `save` open the
  system's dialogs, owned by the window. `Ok(None)` is a cancel; an
  `Err` means no dialog could open (no desktop session, no dialog tool),
  never a cancel. A save dialog asks before replacing a file, so a path
  that comes back is one the person agreed to write.
- **Later launches**: a second start of the app hands its arguments to
  the running one and ends; `aio_shell::launches()` hears them, and
  `Launch::path` makes a relative argument absolute from where it was
  typed. The window comes to the front by itself.
- **Updates**: `aio::update::Updates` is a cell like any other;
  `client!` gives its views their own `part` and `act!`, and each
  `client!` module's `use_live()` goes in the root.

## 3. The launcher

The template's launcher, with the tray and the `updates` cell's source:

```rust,no_run
# use aio::{Cx, Uuid};
# #[aio::cell(persist, coalesce, id = "notes")]
# pub struct Notes { pub n: u32 }
# #[aio::methods]
# impl Notes {}
# use aio::ui::prelude::*;
# aio::client!(Notes, notes);
# #[component]
# pub fn App() -> View { rsx! {} }
# pub fn new_note() {}
# pub mod updates { use aio::ui::prelude::*; aio::client!(aio::update::Updates, updates); }
use aio_shell::Start;
use aio_shell::desktop::{Item, Tray};

static TRAY: [Item; 4] = [
    Item::run("New note", new_note),
    Item::go("Settings", "/settings"),
    Item::SEPARATOR,
    Item::go("All notes", "/"),
];

pub fn launch(cfg: &aio::config::Config, version: &str) -> std::io::Result<()> {
    let Start::Run(dirs) = aio_shell::start(cfg, version)? else { return Ok(()) };
    _ = SOURCE.set(aio::Source::for_app(cfg, &dirs)?);
    if let Some(u) = aio_shell::update::cell() {
        _ = updates::SOURCE.set(u); // set by start when [updates] url is
    }
    let tray = Tray { tooltip: None, close_to_tray: true, menu: &TRAY };
    let icon = None; // Some(include_bytes!("../assets/icon.png")): the window's and the taskbar's
    aio_shell::run(App, aio_shell::Window { width: 1000, height: 700, icon, tray: Some(tray), ..aio_shell::Window::of(cfg) }, &dirs)
}

# fn main() {}
```

In `app/src/main.rs` it is called as
`app::launch(&aio::config!(), aio::app_version!())`, which is the
template's own launcher with these lines moved into the library.

- `aio_shell::start` claims the app's home: the single-instance lock.
  A second launch hands over and returns `HandedOver` (so `launch`
  returns); `--version` and an update being installed return `Done`.
- `Window::of(cfg)` takes the title, `lang` and `dir` from `[app]`; the
  size a person leaves the window at is kept per app, and `width` and
  `height` are only its first size.
- `close_to_tray: true` hides the window on close; the tray's Quit, or
  the app stopping, still closes it. A desktop with no tray minimizes
  it instead. A tray item runs its function in the views, then brings
  the window forward at its route.

## 4. Config, data and shipping

```toml
# aio.toml
[app]
id = "notes"             # its data's home: never change it once it ran
name = "Notes"
mode = "standalone"
version = "1.4"

[updates]
url = "https://example.org/notes"
keys = ["ed25519:REPLACE_WITH_THE_KEY_AX_KEYGEN_PRINTS"]
every = "6h"
auto = false             # offer it: the Banner shows it, the person decides

[capabilities]
network = ["example.org"]
```

```sh
ax dev notes                      # the window; edits hot-patch in
ax dev notes --size 390x844       # a phone-sized window, for this run
ax test notes                     # the cell's tests and the window's, headless
ax data notes                     # where the notes live, and what to back up
ax backup notes notes.bak         # a consistent copy, safe while it runs
ax keygen notes                   # once: the signing key and the [updates] keys line
ax build notes                    # this OS's bundle in dist/
ax release notes --channel stable --notes "Search in bodies"
```

- The notes are in the app's home (`~/.local/share/aio-rt/apps/notes/`
  on Linux; `ax data notes` says where on each OS), in `data/state.db`.
- `ax build` makes this OS's installable bundle: a Linux `.tar.gz` with
  `install.sh` (`--deb`, `--appimage` for the others), a macOS `.app`, a
  Windows one-click `.exe`; the window runs on aio-rs's shared Electron
  runtime, fetched once per machine ([build](../build/README.md)).
- With `auto = false` a release is offered, never installed by itself:
  the `updates` cell's phase becomes `Available`, the Banner shows it,
  and `apply` installs it and restarts. A release that fails to start
  is rolled back to this one ([release](../build/release.md)).

## 5. Tests

- **The cell**: section 1's test, on `TestCells` (effects settle at
  once, files in a temp folder).
- **The window**: `TestUi::new(App)` mounts it headless in the real
  page; `ui.press("body", "ctrl+n")` is the shortcut, `ui.click("new")`
  the button ([testing](../testing/README.md)).
- **The dialogs** need a desktop, so a test drives what comes after
  them: call `export` and `import` with a path, as section 1 does.
- **The update banner**: `aio_shell::update::stub("1.4.0",
  Found::Offer(..))` makes `aio_shell::update::cell()` a stubbed cell;
  put it in `updates::SOURCE` before mounting, and `check` offers it.

## What maps to what

| aio's Electron notes app | Here |
|---|---|
| `db: { notes: table({ .. }) }` | a `persist` cell (or `#[table]` on the field for SQL) |
| `visible.forUser` to send only the active note | not needed: the window is in the app's process, views read fields with `part` |
| `<Route path="/note/:id">`, `useRoute`, `Link` | `aio::routes!`, `aio::router!` (`here()`, `Link`) |
| `singleton: true` | always: `aio_shell::start` holds the lock; `--takeover` replaces the running one |
| `ui: { title, width, height }` | `aio_shell::Window { width, height, ..Window::of(&cfg) }` |
| `--compile --electron` | `ax build notes` |
| `testCell(notes, ..)` | `TestCells`; `TestUi` for the window |

## Limits

- No app menu bar of the app's own: the window has none on Linux and
  Windows, and on macOS the system's standard menus (app, Edit, Window).
  Menus are in the page (`aio::kit::Menu`) and the tray.
- The app has one window of its own views; `aio_shell::open_window`
  opens child windows for other sites only (`Window { web }`).
- A dialog asked for with no window to own it (before the window is up)
  is the desktop's own tool: `zenity` or `kdialog` on Linux.
- Windows and macOS bundles are not OS-signed yet: SmartScreen and
  Gatekeeper warn on first open ([release](../build/release.md)).

## Common mistakes

- Reading the file in the method: a method does no I/O. Ask for an
  effect, as `import` does.
- Treating `Err` from a dialog as a cancel: it is a broken dialog; say
  so (a toast), and keep `Ok(None)` for the cancel.
- A tray item list built in a function: `Tray::menu` is `&'static`; make
  it a `static`.
- `use_live()` of the `updates` module missing in the root: the banner
  never sees the offer.
- Paths in the cell from `std::env::home_dir()` or the like: a test or a
  profile then writes to the real home. Ask `aio::paths::app_dirs()`.

## API

`cargo doc -p aio-rs-shell --no-deps --open`: `start`, `Start`, `run`,
`Window`, `page` (`pick`, `pick_many`, `save`, `Pick`, `Save`),
`desktop` (`Tray`, `Item`, `use_asks`, `notify`), `launches`, `Launch`,
`update` (`cell`, `stub`); `cargo doc -p aio-rs --no-deps --all-features
--open`: `aio::kit` (`Menu`, `GlobalKeys`, `use_global_key`,
`ToastHost`, `toast`), `aio::routes!`, `aio::router!`, `aio::update`,
`aio::paths::write_atomic`, `aio::tell`.

# Quickstart

From nothing to a running, tested and packaged app in five commands.

```sh
ax setup            # once per machine: checks Rust and the C toolchain, fetches the runtime
ax create notes     # a new app in ./notes (a window on a saved cell)
cd notes
ax dev notes        # runs it; edits hot-patch in
ax test notes       # its tests, the UI test included
ax build notes      # an installable app for this OS in dist/
```

Installing `ax` itself: [setup](setup.md). The app takes aio-rs from git at
this `ax`'s release, so a clone builds anywhere and no aio-rs checkout is
needed ([where an app gets aio-rs from](setup.md#where-an-app-gets-aio-rs-from)).

## Before you start

- Rust (rustup) and a C compiler and linker: [setup](setup.md#1-rust-and-a-c-toolchain).
- `ax` on `PATH`: `ax version` prints its version.
- `ax setup` once: it fetches the shared Electron runtime every window and
  UI test uses, and dx for hot-patching. `ax setup --check` says what is
  missing without fetching; the exit code is 1 while a needed piece is.
- Nothing else: no Node, no system browser, no OpenSSL.

## ax create

```sh
ax create notes                      # a window app (the default template)
ax create notes --template cli       # a command-line tool on a saved cell
ax create board --template service   # cells served to clients, no window
ax create notes --id notes-dev       # its id apart from its name (its data's home)
ax create notes --aio-version 0.2.0  # another aio-rs release (main, latest)
ax create notes --aio ../aio-rs      # an aio-rs checkout instead, named relative to the app
ax create notes --target deb,android # its [build] targets
ax create notes --css tailwind       # Tailwind writes its stylesheet
ax create notes --force              # into a folder that holds files already
```

| Option | What it does | Default |
|---|---|---|
| `<name>` | the folder, the package, the binary: lowercase letters, digits and `-`, starting with a letter, not `app` or ending in `-app` | – |
| `--template` | `app`, `cli` or `service` | `app` |
| `--id` | `[app] id`: where its data lives; refused when an installed app has it (the two would share data) | the name |
| `--aio-version` | the aio-rs release its `Cargo.toml` takes from git: `0.2.0`, `main` (the branch), `latest` (the newest release) | this `ax`'s version |
| `--aio` | an aio-rs checkout (`crates/aio`, `crates/aio-shell`) instead, named relative to the app; the two share a folder below the root | – |
| `--target` | `[build] targets`: the kinds `ax build` makes (`ax build <app> --list`); `android` a window app's | none: this OS's bundle |
| `--css tailwind` | `[build] css` runs `tailwindcss -i src/app.css -o assets/style.css` before every build; `src/app.css` starts with the template's styles (window app) | – |
| `--force` | write the template's files into a folder that isn't empty (only those files) | refused |

- A name another app already builds in ax's shared build folder is
  refused: two apps with one binary name would overwrite each other there.
- When an app with that id already keeps data on this machine, `ax create`
  says so: the new app opens that data.
- It writes `Cargo.lock` too, makes a git repository with a first commit
  (not inside another repository), so the first build is a clean release.
- A failure part-way leaves nothing behind (with `--force`, the files it
  wrote over are put back). `--json`: the files and the next steps.
- Nothing of the machine it was made on is in the app (no folder of yours,
  no checkout's absolute path): a clone builds anywhere.

### The three templates

| Template | `[app] mode` | Features | `src/lib.rs` holds | Runs as |
|---|---|---|---|---|
| `app` | `standalone` | `kit` (and `web` for its browser build) | the `Counter` cell, its window `App`, a UI test | a desktop window; `ax dev --web`, `ax build --android` for the other targets |
| `cli` | `cli` | none | the `Notes` cell, `run` (one command on it), a test | `notes add buy milk`, `notes list`, `notes done 1` |
| `service` | `server` | `net` | the `Board` cell (`shared`), a `#[client]` method, a test | a headless server on `[server] listen` (`127.0.0.1:7420`) |

## What you got

```text
notes/
├── aio.toml              the app's one config: [app] id, name, mode, version…
├── Cargo.toml            the workspace and the app's library crate
├── src/lib.rs            the app: its cell and its window (ax dev hot-patches it)
├── assets/style.css      its styles (a saved edit shows at once; app only)
├── app/Cargo.toml        the launcher's crate: the binary named like the app
├── app/src/main.rs       the launcher: boots the library
├── app/src/lib.rs        the same window on Android (app only)
├── rust-toolchain.toml   aio-rs's Rust, so one build folder serves every app
├── .cargo/config.toml    the bundled SQLite without the extensions aio never uses
├── .gitignore            target/, .data/, dist/, .env, release keys
├── .gitattributes        LF line endings in every checkout
├── README.md             run, test, build, ship; where its aio-rs comes from
├── AGENTS.md, CLAUDE.md  for coding agents: the rules, the loop, ax agent
└── Cargo.lock
```

What each file is for, the launcher, and the Cargo features:
[project](project.md).

## The cell

`src/lib.rs` starts with the state and the only ways to change it:

```rust
#[aio::cell(persist, coalesce, id = "counter")]
pub struct Counter {
    pub count: i64,
}

#[aio::methods]
impl Counter {
    /// Count `by` more (or less).
    pub fn add(&mut self, by: i64) {
        self.count += by;
    }

    pub fn reset(&mut self) {
        self.count = 0;
    }
}

# fn main() {
use aio::Handle;
let h = Handle::<Counter>::memory();
h.add(2).unwrap();
h.add(-1).unwrap();
assert_eq!(h.state().count, 1);
# }
```

- `persist` saves it in the app's `state.db` with an event per call;
  `coalesce` shows each call at once and writes within 100 ms.
- `id = "counter"` is where its data is: never change it once the app
  has run.
- A method takes `&mut self` and arguments that are JSON-able, and changes
  the state; nothing else may. One that can refuse returns `aio::Result`
  (`ensure!`, `reject!`), and a refused call changes nothing.
- Every call is in the log: `ax events notes` shows it, `ax state notes
  counter` the state the running app holds, `ax call notes counter add 5`
  calls it from the terminal.

## The window

```rust,ignore
// ax create's template (src/lib.rs), compiled by ax's own tests:
aio::client!(Counter, counter);

#[component]
pub fn App() -> View {
    use_live();
    let n = part(|c| &c.count);
    let theme = aio::kit::theme_css(aio::hue!());
    rsx! {
        style { {aio::use_css!(aio::ui::scheme(), &theme; "assets/style.css")} }
        main {
            h1 { "data-t": "count", "{n}" }
            button { class: aio::ui::btn("primary"), onclick: move |_| _ = act!(add(1)), "Add" }
            button { class: aio::ui::btn("ghost"), onclick: move |_| _ = act!(add(-1)), "Subtract" }
            button { class: aio::ui::btn("ghost"), disabled: n.get() == 0, onclick: move |_| _ = act!(reset()), "Reset" }
        }
    }
}
```

- `aio::client!` connects the views to the cell (it makes `SOURCE`,
  `part`, `use_live` and the rest for this cell).
- `part(|c| &c.count)` is a handle on one field: the view reading it
  (`"{n}"`, `n.get()`) re-renders only when it changes.
- `act!(add(1))` calls a method from a handler.
- `aio::use_css!` reads `assets/style.css`; under `ax dev` a saved edit
  shows at once, with no compile.

The launcher opens the window at a size of its own; `ax dev notes --size
390x844` tries another size for one run (the window's saved size stays):

```rust,ignore
// ax create's launcher (app/src/main.rs), compiled by ax's own tests:
fn main() -> std::io::Result<()> {
    let cfg = aio::config!();
    let Start::Run(dirs) = aio_shell::start(&cfg, aio::app_version!())? else { return Ok(()) };
    _ = app::SOURCE.set(aio::Source::for_app(&cfg, &dirs)?);
    aio_shell::run(app::App, aio_shell::Window { width: 900, height: 640, ..aio_shell::Window::of(&cfg) }, &dirs)
}
```

## The test

The template's test drives the real page headless, in the pinned runtime:

```rust,ignore
// ax create's template (src/lib.rs), compiled by ax's own tests:
#[test]
fn the_buttons_count_and_reset() {
    let mut ui = TestUi::new(App);
    assert_eq!(ui.text("count"), "0");
    assert!(ui.disabled("ResetButton"), "nothing to reset");
    ui.click("AddButton");
    ui.click("AddButton");
    ui.click("SubtractButton");
    assert_eq!(ui.text("count"), "1");
    ui.click("ResetButton");
    assert_eq!(ui.text("count"), "0");
}
```

`ResetButton` is the name the button's text gives it; `data-t` pins a name
that survives a reworded label (`"count"`). A cell alone is tested with no
page at all:

```rust
# #[aio::cell(id = "counter")]
# pub struct Counter { pub count: i64 }
# #[aio::methods]
# impl Counter {
#     pub fn add(&mut self, by: i64) { self.count += by; }
# }
# fn main() {
let h = aio::Handle::<Counter>::memory();
h.add(5).unwrap();
assert_eq!(h.state().count, 5);
# }
```

Every harness runs in a sandbox of its own: no test reaches the app's real
data ([testing](../testing/README.md)).

## The other templates

### cli

```rust
use aio::Door;

#[aio::cell(persist, id = "notes")]
pub struct Notes {
    pub items: Vec<String>,
}

#[aio::methods]
impl Notes {
    /// Keep a note; an empty one is refused.
    pub fn add(&mut self, text: String) -> aio::Result {
        aio::ensure!(!text.trim().is_empty(), "a note needs some text");
        self.items.push(text);
        Ok(())
    }

    /// Forget note number `n` (from 1, as `list` shows them).
    pub fn finish(&mut self, n: usize) -> aio::Result {
        aio::ensure!((1..=self.items.len()).contains(&n), "there is no note {n}");
        self.items.remove(n - 1);
        Ok(())
    }
}

pub const USAGE: &str = "usage: add <text…> | done <n> | list";

/// One command on the cell behind `door`: what to print, or the exit code
/// and why not (1: refused, 2: bad usage).
pub fn run(door: &impl Door<Notes>, args: &[String]) -> Result<String, (u8, String)> {
    let refused = |e: aio::Rejected| (1, e.why);
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["add", text @ ..] if !text.is_empty() => door.add(text.join(" ")).map(|_| "kept".into()).map_err(refused),
        ["done", n] => {
            let n = n.parse().map_err(|_| (2, format!("done {n}: a note's number\n{USAGE}")))?;
            door.finish(n).map(|_| "done".into()).map_err(refused)
        }
        ["list"] | [] => Ok(door.read(|s| {
            s.items.iter().enumerate().map(|(i, t)| format!("{}. {t}", i + 1)).collect::<Vec<_>>().join("\n")
        })),
        _ => Err((2, USAGE.into())),
    }
}

# fn main() {
let h = aio::Handle::<Notes>::memory();
let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
assert_eq!(run(&h, &s(&["add", "buy", "milk"])), Ok("kept".into()));
assert_eq!(run(&h, &s(&["list"])), Ok("1. buy milk".into()));
assert_eq!(run(&h, &s(&["done", "7"])), Err((1, "there is no note 7".into())));
assert_eq!(run(&h, &s(&["fly"])), Err((2, USAGE.into())));
# }
```

`run` takes any door (`impl Door<Notes>`), so the test hands it a memory
`Handle` and the launcher the tool's saved cell; the launcher prints the
answer and exits with the code. A real tool parses its arguments with
[`aio::cli`](../clients/cli-apps.md).

```sh
cargo run -q -p notes-app -- add buy milk   # one command, then out
cargo run -q -p notes-app -- list            # 1. buy milk
ax build notes                               # dist/: the binary with install.sh
```

### service

```rust
#[aio::cell(persist, shared, id = "board")]
pub struct Board {
    pub posts: Vec<Post>,
}

/// A type the cell keeps is `#[aio::data]` (saved and sent).
#[aio::data]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Post {
    pub by: aio::By,
    pub text: String,
}

#[aio::methods]
impl Board {
    /// Post `text` as the caller.
    #[client]
    pub fn post(&mut self, cx: &aio::Cx, text: String) -> aio::Result {
        aio::ensure!(!text.trim().is_empty(), "a post needs some text");
        aio::ensure!(text.chars().count() <= 500, "a post is 500 characters at most");
        self.posts.push(Post { by: cx.by().clone(), text });
        Ok(())
    }
}

# fn main() {
let h = aio::Handle::<Board>::memory();
let ann = h.by(aio::By::User("ann".into()));
ann.post("hello".into()).unwrap();
assert!(ann.post(" ".into()).is_err());
assert_eq!(h.state().posts[0].by, aio::By::User("ann".into()));
# }
```

- `shared` sends the state, and every change, to each client; only
  `#[client]` methods may be called over the network.
- `cx.by()` is who called: the signed-in user, a device, `ax`, an effect.
- Its launcher serves until it is asked to stop, and returns the exit
  code a supervisor reads:

```rust,ignore
// ax create's template (app/src/main.rs, --template service), compiled by ax's own tests:
let cfg = aio::config!();
let Start::Run(dirs) = aio_shell::start(&cfg, aio::app_version!())? else {
    return Ok(std::process::ExitCode::SUCCESS);
};
let _board = aio::Source::<app::Board>::for_server(&cfg, &dirs)?;
aio_shell::serve(&dirs)?;
Ok(aio::lifecycle::exit_code())
```

```sh
ax start board              # in the background, until it answers
ax call board board post hello
ax state board board
ax stop board
```

Clients, pairing and the network: [sync](../state/sync.md); running it as
a service: [setup](setup.md#run-at-startup-or-as-a-service).

## Run it

| Command | What it does |
|---|---|
| `ax dev notes` | build, run, and hot-patch each saved edit in (dx); without dx, rebuild and restart |
| `ax dev notes --fresh` | the same on new, empty data |
| `ax dev notes --instance try` | a private copy with data of its own |
| `ax dev notes --size 390x844` | the window at that size for this run |
| `ax dev notes --web` | its browser build on loopback, the tabs live |
| `ax dev notes --android` | on the attached phone or emulator, again on each saved change |
| `ax start notes` / `ax stop notes` | in the background, then closed (scripts, agents) |

Run from a terminal with no person at it (an agent), the window goes on a
nested display, never the person's screen (`--display auto`).

## Look at it running

While `ax dev notes` runs, from another terminal:

```sh
ax state notes counter          # the state the running app holds
ax call notes counter add 5     # a call, as the window's button makes it
ax events notes --last 5        # the newest events in the log
ax surface notes                # what a person sees, named as TestUi names it
ax shot notes                   # a PNG of the window
ax logs notes --level warn      # its warnings and errors
ax stop notes                   # close it
```

## Test it

```sh
ax test notes                   # cargo test, in ax's shared build folder
ax test notes buttons           # the tests whose name holds "buttons"
ax test notes --video out/      # a video of each UI test (ffmpeg)
ax lint                         # what types can't check: impure methods, secrets, a11y
```

UI tests need no display: they draw headless in the pinned runtime.

## Build it

```sh
ax build notes                  # this OS's installable app in dist/
ax build notes --deb            # a Debian package ([app] maintainer)
ax build notes --appimage       # one portable Linux file
ax build notes --windows        # Windows, cross-built from Linux
ax build notes --web            # an installable web app (PWA)
ax build notes --android        # signed APKs
ax install notes                # build this OS's bundle and install it for this user
```

The files are named `dist/<id>-<version>-<os>-<arch>.*`: on Linux a
`.tar.gz` with `install.sh` and `uninstall.sh`, on macOS a zipped `.app`,
on Windows a one-click `.exe` and a `.zip`. The version is `[app]
version` plus the commit count (`0.1.<commits>`; uncommitted edits add
`-dirty.<hash>`, no git `0.1.0-nogit.<hash>`) ([versioning](versioning.md)). Every target:
[targets](../build/targets.md); updates and signing:
[release](../build/release.md).

## Troubleshooting

First, ask ax: `ax setup --check` (a tool missing), `ax errors notes` (the
failed build, then the newest errors), `ax doctor notes` (is it the code
on disk, who decided each setting).

| Symptom | Cause | Fix |
|---|---|---|
| `ax create` refuses: no aio-rs checkout | a prebuilt `ax` doesn't know where aio-rs's source is | `ax create notes --aio <checkout>` |
| `ax create` refuses the name | not lowercase letters, digits and `-`, or `app` / `…-app` | `notes`, `my-notes` |
| `ax create` refuses the id | an installed app has it | `--id notes-dev`, or `ax remove notes` first |
| the build fails naming an `aio.toml` key | `aio.toml` is parsed as the app builds | fix that key ([reference](../reference/aio-toml.md)) |
| `cc` or `link.exe` not found | no C toolchain | [setup](setup.md#1-rust-and-a-c-toolchain) |
| the window never opens (Linux) | Electron's system libraries are missing | install what `ax setup` names |
| `ax dev` rebuilds on every edit | no dx, or an edit dx can't patch (a new dependency, a changed type) | `ax setup` fetches dx; the rest is expected |
| a CSS edit doesn't show | the CSS is kept in `once` (made once) | `aio::use_css!` in the render |
| the app starts with no data | `[app] id` or the cell's `id` changed | change it back |
| a second `ax dev` shows nothing new | the running app took the launch and the new one ended | stop the first, or `--instance <n>` |
| a service won't start: the port is in use | another process has `[server] listen`'s port | stop it, or `--port <n>` for this launch |
| the version ends in `-dirty.…` or `-nogit.…` | uncommitted edits, or no git repository | commit; `git init` gives build numbers |

More, per OS: [troubleshooting](../debugging/troubleshooting.md).

## Next

- Build a small app step by step: [tutorial](tutorial.md).
- What a cell, a method, an effect and a view are: [concepts](concepts.md).
- The layout, `aio.toml` and features: [project](project.md), every key:
  [aio.toml reference](../reference/aio-toml.md).
- Every verb: [ax](../clients/ax.md).
- Coming from aio: [migration](../migration/README.md).

<p align="center">
  <img src="docs/aio-rs-logo.svg" alt="aio-rs" width="380">
</p>

<p align="center">
  <b>Write the state in Rust. Get the app.</b><br>
  A cell is your state, its saved history, its sync and what your views
  read: one app as a desktop window, a web page, an Android app, a
  service or a command-line tool. Its command is <code>ax</code>.
</p>

<p align="center">
  <code>0.4.2</code> · <b>early ALPHA</b> · <a href="LICENSE">MIT</a> ·
  <a href="docs/README.md">Docs</a> ·
  <a href="docs/basics/quickstart.md">Quickstart</a> ·
  <a href="CHANGELOG.md">Changelog</a>
</p>

---

**aio-rs is early ALPHA** (0.4.2, its sixth release); until 1.0.0 a
minor version may break things ([versions](docs/basics/versioning.md)).

**Read first: [when to use aio-rs, and when not](docs/basics/when.md).**
Yes for an app with a window and state; no for SEO pages, a CDN or the
edge. Pin a release tag; experimental parts are marked there.

### aio or aio-rs?

aio-rs is the Rust successor of [aio](https://github.com/riagentic/aio),
the TypeScript/Deno line (command `am`), which is the mature beta line.
Pick **aio** for a frozen 1.x surface, an edit loop that runs from source
in ~3 s at any size, more features, and an iOS client. Pick **aio-rs**
for Rust: types checked at compile time from the cell to the view, a
measured faster UI with about 3.5× less memory, one native binary per
app; it is early alpha and has fewer features. Both read the same app
data (`state.db`) and speak the same wire, so an app can move one piece
at a time: [aio and aio-rs compared](docs/aio-vs-aio-rs.md),
[the migration guide](docs/migration/README.md).

## ⚡ Start

You need Rust ([rustup](https://rustup.rs)) and a C toolchain; per OS:
[setup](docs/basics/setup.md). From source, on Linux, Windows or macOS,
at the release tag (an app takes aio-rs from git at its `ax`'s release, so
no checkout is needed afterwards):

```sh
git clone --branch v0.4.2 https://github.com/riagentic/aio-rs
cargo install --locked --path aio-rs/crates/aio-cli   # ax 0.4.2
ax setup                                             # checks the machine, fetches the runtime
```

Or a prebuilt `ax` for Linux x86_64 and macOS arm64 and x86_64 (Windows
x86_64: `install.ps1` from the
[release](https://github.com/riagentic/aio-rs/releases/tag/v0.4.2)), each
archive's SHA-256 pinned in the installer:

```sh
curl -fsSL https://github.com/riagentic/aio-rs/releases/download/v0.4.2/install.sh | sh   # then runs ax setup
```

An `ax` built from `main` writes `main`'s templates but pins its release
tag: build on the tag as above, or make the app on that checkout
(`ax create notes --aio <checkout>`).

Then:

```sh
ax create notes      # a new app in ./notes: a window on a saved cell
cd notes
ax dev notes         # run it; edits hot-patch in, the state kept
ax test notes        # its tests, the UI test included
ax build notes       # an installable app for this OS in dist/
```

> **Working with a coding agent?** Have it run **`ax agent`** first: the
> model, the verbs and the rules in one page. Every new app gets an
> `AGENTS.md` and a `CLAUDE.md` that say the same.

One flag from the rest: `ax dev notes --web` (the web build),
`ax build notes --android` (an APK), `ax build notes --deb --appimage`,
`ax build notes --windows`.

## 🧠 The idea

State lives in a cell, changed only by its methods. Each call is an
event in a log: saved, replayable (`ax replay`, `ax timetravel`), synced
to every copy.

```rust
#[aio::cell(persist, id = "counter")]
pub struct Counter {
    pub count: i64,
}

#[aio::methods]
impl Counter {
    /// Count `by` more (or less): one event in the log, saved.
    pub fn add(&mut self, by: i64) {
        self.count += by;
    }
}

# fn main() {
let h = aio::Handle::<Counter>::memory();
h.add(2).unwrap();
assert_eq!(h.state().count, 2);
# }
```

A view reads the fields it shows and calls methods with `act!`:

```rust,ignore
// ax create's template (src/lib.rs), compiled by ax's own tests:
aio::client!(Counter, counter);

#[component]
pub fn App() -> View {
    use_live();
    let n = part(|c| &c.count);
    rsx! {
        main {
            h1 { "data-t": "count", "{n}" }
            button { class: aio::ui::btn("primary"), onclick: move |_| _ = act!(add(1)), "Add" }
        }
    }
}
```

`part(|c| &c.count)` is a handle on that field: the view's body runs
once, and only the text that reads it (a hole) updates when it changes. Views are written against `aio::ui`
(`use aio::ui::prelude::*`), which names no renderer.
`count` is saved to SQLite, comes back on restart, and is checked by the
compiler everywhere it is used: a misspelt field is a build error.

## 🧪 And its test

The template's test drives the real page headless, in the pinned
runtime; names come from the buttons' text or a `data-t`:

```rust,ignore
// ax create's template (src/lib.rs), compiled by ax's own tests:
#[test]
fn the_buttons_count_and_reset() {
    let mut ui = TestUi::new(App);
    assert_eq!(ui.text("count"), "0");
    ui.click("AddButton");
    ui.click("AddButton");
    ui.click("SubtractButton");
    assert_eq!(ui.text("count"), "1");
}
```

The running app is drivable too: `ax surface` prints what is on screen,
`ax trigger` acts on it, `ax state` and `ax call` read and call its cells.

## 📦 What you get

| | |
|---|---|
| 💾 **Data** | SQLite with an event log · replay and time travel · migrations · coalesced writes · backups · SQL tables and blobs · offline queue |
| 🎨 **UI** | `aio::ui` views with field-level reads · the kit: controls, keys, windowed lists, forms, dialogs, toasts · themes, live CSS · routing · islands |
| 🔐 **Auth** | the app's key · accounts, roles, sessions · TOTP · OIDC · PAKE pairing · TLS on every exposed launch |
| 🧪 **Testing** | `TestCells` (virtual time, seeded fuzzing) · `TestUi` (the real views, headless) · `TestServer` · cassettes · `ax lint` |
| 🚚 **Ship** | desktop (Linux, Windows, macOS) · web (wasm, PWA) · Android · service · CLI · signed delta updates, gated on the data, rolled back on a failed start |
| 🛠️ **Operate** | `ax`: status, health, logs, state, call, events, sql, heap, doctor, problem reports |

Guide: [docs/](docs/README.md); every Rust example in it compiles and
runs as a test. API: `cargo doc -p aio-rs --no-deps --open`. Next to
Tauri and Dioxus, measured: [comparison](docs/comparison.md). Why not
plain Rust: [why aio-rs](docs/basics/why.md).

## 🎯 Honestly

- 🧊 **Early ALPHA.** A minor version may break things until 1.0.0; the
  [changelog](CHANGELOG.md) says how to move. Every change to the public
  API shows in review: `./ci.sh` checks it against a committed snapshot.
- 📏 **What is gated, what is not.** Every release passes `./ci.sh` on
  Linux (fmt, dependency and docs checks, clippy for every feature set,
  wasm, Windows and Android, the whole test suite, a measured template
  app) and the Windows lane on a real Windows machine; the Mac runs the
  workspace tests and smoke-tests the template app; the Android gate
  (`scripts/android-gate`) starts a sample APK on an emulator and upgrades
  it with its data kept; GitHub CI installs a sample app on Linux, macOS
  and Windows, updates it through a signed patch and rolls a broken
  update back (`scripts/update-proof`). The README's numbers rerun with
  `scripts/numbers`. Not gated: a physical Android device and
  OS-signing of bundles (SmartScreen and Gatekeeper warn on first start).
- 🚧 **Not there yet.** No app running on iOS itself (`ax build --ios`
  makes a thin client of a server, as aio's), crates not on crates.io (an
  app takes aio-rs from git). A Rust edit recompiles its crate: keep a big
  app in page crates
  ([what an edit costs](docs/build/README.md#what-an-edit-costs)).
- ✅ **Built for** apps where the state is the product: desktop tools,
  local-first apps, small servers and the clients that sync with them,
  command-line tools on saved state.
- ❌ **Not for** server-rendered, SEO-first pages or static sites, state
  redrawn every frame, one state across many servers, or a native iOS
  app: each app is one process ([FAQ](docs/basics/faq.md)).

## License

[MIT](LICENSE)

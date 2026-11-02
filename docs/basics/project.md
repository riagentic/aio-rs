# Project layout, aio.toml and features

## Layout

An app is its own Cargo workspace with two crates, made by `ax create`:

| Path | What it is |
|---|---|
| `Cargo.toml` | the workspace, and the app's library crate (`[package] name` = the app's name) |
| `src/lib.rs` | the app: cells, views, effects (split into modules as it grows) |
| `app/Cargo.toml`, `app/src/main.rs` | the launcher binary: it only boots the library, so `ax dev` can hot-patch all of the app |
| `app/src/lib.rs` | the same window on Android (`ax build --android`; window apps only) |
| `aio.toml` | the app's one config file |
| `assets/` | stylesheets and files the views use (window apps only) |
| `rust-toolchain.toml` | aio-rs's Rust, so ax's shared build folder is reused across apps |
| `.cargo/config.toml` | the build's settings: the bundled SQLite without the extensions aio never uses |
| `.gitignore` | `target/`, `.data/`, `dist/`, `.env` and release keys |
| `.gitattributes` | LF line endings in every checkout, whatever git's `core.autocrlf` |
| `README.md` | run, test, build, ship; where its aio-rs comes from |
| `AGENTS.md`, `CLAUDE.md` | for coding agents: the rules and the loop, then `ax agent` and the guide by URL |

Tests live beside the code (`#[cfg(test)] mod tests`) or in `tests/`.

### Names

`ax create <name>`: lowercase letters, digits and `-`, starting with a
letter, not `app` and not ending in `-app` (the launcher's package is
`<name>-app`; its binary is `<name>`). The name is the app's binary, its
library crate and, unless `--id` says otherwise, its `[app] id`.

### The app's root

`ax` finds an app as a binary of the Cargo workspace of the current
folder (`cargo metadata`): `ax dev notes` builds and runs the binary
`notes`; a `[build] targets` label works too. The app's root is the
folder of its `aio.toml`: the nearest one from the launcher's folder up,
the same file `aio::config!()` builds in (`aio::config!("../aio.toml")`
names another). Its `assets/` sit beside it.

## Where code goes

| What | Where |
|---|---|
| a cell, its methods, its tests | its own module: `src/todo.rs` (`#[cfg(test)] mod tests` at the end) |
| views of one cell | beside it (`src/todo/view.rs`), or `src/ui/` when views span cells |
| the root component and `routes!` | `src/lib.rs` |
| effects' I/O helpers (HTTP, files) | a module the methods' effects call (`src/io.rs`); never called from a method |
| types two cells share | `src/types.rs`, `#[data]` |
| code only the native build has | behind `#[cfg(not(target_arch = "wasm32"))]` ([targets](../build/targets.md#web)) |
| the launcher | `app/src/main.rs`: boot only, no app logic |
| styles, icons, fonts | `assets/` |
| integration tests (`TestServer`, several cells) | `tests/` |

A small app is `src/lib.rs` alone, as the template writes it:

```text
notes/
  aio.toml
  Cargo.toml
  src/lib.rs          # the cell, its tests, the views
  app/src/main.rs
  assets/style.css
```

A larger one splits by domain, one module per cell:

```text
shop/
  aio.toml
  Cargo.toml
  src/lib.rs          # mod declarations, App, routes!
  src/catalog.rs      # #[cell] Catalog + its tests
  src/cart.rs         # #[cell] Cart + its tests
  src/cart/view.rs    # the cart's components
  src/io.rs           # the payment provider's client, used by effects
  src/types.rs        # #[data] Money, Sku
  tests/checkout.rs   # catalog + cart + a server, end to end
  app/src/main.rs
  assets/
```

Growing:

- One module per cell once there are two cells; one file per screen once
  a view passes a couple of hundred lines.
- A second crate (a workspace member) only when another app or a tool
  needs the code too: cells and types in a library both depend on.
- Cell ids and `[app] id` never change while you move code: they name the
  data, not the module.

### Several apps in one repository

One Cargo workspace may hold several apps: each is its own launcher
package with its own `aio.toml` (its own `[app] id`, so its own data,
lock and log), and code they share is a library crate of the workspace
that both depend on (a path dependency). `ax dev <binary>` picks the app;
`ax` watches the launcher's package and the workspace packages it uses.
For a service and its clients: [architecture](architecture.md#2-service-and-rich-clients).

What aio has and aio-rs needs no form of: `share` (shared folders are
crates), import rules and `*.server.ts` naming (the compiler resolves
imports; `cfg` keeps native code out of the page).

## The launcher

```rust,ignore
// ax create's launcher (app/src/main.rs), compiled by ax's own tests:
fn main() -> std::io::Result<()> {
    let cfg = aio::config!();
    let Start::Run(dirs) = aio_shell::start(&cfg, aio::app_version!())? else { return Ok(()) };
    _ = app::SOURCE.set(aio::Source::for_app(&cfg, &dirs)?);
    aio_shell::run(app::App, aio_shell::Window { width: 900, height: 640, ..aio_shell::Window::of(&cfg) }, &dirs)
}
```

- `aio::config!()` parses the nearest `aio.toml` as the app *builds*: a
  mistake is a compile error, and the binary holds no TOML parser.
- `aio_shell::start` claims the app's home (a second launch hands over to
  the first and ends there), starts its log, health and problem reports.
- `aio::Source::for_app` opens the first cell as `[app] mode` says; more
  cells open on the same store with `Handle::open` (a port of an aio app:
  `open_aio`, [data](../migration/data.md)).
- A server: `aio::Source::for_server(&cfg, &dirs)` then
  `aio_shell::serve(&dirs)`. A CLI tool: the cell, then `aio::cli::args`.

What varies per launch is a flag or a variable, never a config file read
at run time: `--home`, `--profile`, `--instance`, `--port`, `--host`,
`--expose`, `AIORT_DATA`, `AIORT_TOKEN`, `AIORT_UPDATE_CHANNEL`. The boot
report in the log says each setting and who decided it; `ax doctor <app>`
and `ax config <app>` print them.

## aio.toml

The app's one config file, parsed as the app builds (`aio::config!()`):
every key but `[app] id` has a default, an unknown key or a wrong value is a compile
error naming it, and nothing is read at run time. `ax config <app>` prints
it as the build bakes it in, defaults included. The file `ax create`
writes:

```toml
[app]
id = "notes"            # its data's home: never change it once it ran
name = "notes"
mode = "standalone"     # standalone | client-server | client | server | cli
version = "0.1"         # major.minor: ax numbers builds from git (0.1.<commit count>)
lang = "en"             # the page's language, <html lang>
dir = "ltr"             # ltr | rtl | auto
maintainer = "Example Team <team@example.org>"   # the .deb's maintainer

[history]
keep = "90d"            # how long the event log keeps every step

[capabilities]
network = []            # hosts aio may reach besides this machine
```

Every section and key, with its default: [aio.toml reference](../reference/aio-toml.md).

## Cargo features

aio-rs is one crate (`aio-rs`, used as `aio`) with features; an app picks
one fixed set (its template's), because each other set is one more build
of aio-rs.

| Feature | Adds |
|---|---|
| none | the core: cells, the log, replay, persistence, the local door, `aio::cli` |
| `testing` | `aio::testing`, the test kit (`TestCells`; `TestServer` with `server`, `TestUi` with `kit`): a dev-dependency only, so the app's builds leave it out |
| `kit` | the UI kit, `client!`, `router!` |
| `net` | `tls` + `oidc`: everything networked |
| `server` | sync and its server, HTTP routes, server functions, accounts' doors |
| `tls` | the server over TLS, pairing, LAN discovery |
| `oidc` | sign-in with an identity provider |
| `outbound` | aio's HTTPS client, held to `[capabilities] network` |
| `br` | Brotli answers beside gzip |
| `imbl` | `aio::imbl` persistent collections |
| `web` | the app in the browser's page (`ax build --web`) |
| `compat` | talking to aio (Deno) live: aio pages and tools on `/ws`, `aio::compat::AioLink`, `#[on_aio]` ([migration](../migration/README.md)); a ported app's `state.db` needs none of it |
| `dev` | `ax dev`'s hot-patching (never in a release; `ax dev` turns it on) |

The templates: `app` uses `kit` (and `web` for its browser build),
`service` uses `net`, `cli` none; each gives its tests `testing` as a
dev-dependency. A feature is added in the app's `Cargo.toml`:

```toml
aio = { package = "aio-rs", path = "<aio-rs checkout>/crates/aio", features = ["kit", "net"] }
```

`aio-rs-shell` has two features of its own: `dev` (what `ax dev` builds
with: live `rsx!` reloads, Rust hot-patched) and `web` (`aio_shell::web`:
the app's page served to browsers, its first paint rendered per viewer).

## Common mistakes

- App logic in the launcher (`app/src/main.rs`): `ax dev` hot-patches the
  library, not the launcher, and tests don't see it. Keep `main` to boot.

- Changing `[app] id` or a cell's `id` after the app has run: the app
  starts on empty data (ids are where the data is). Change them back.
- Reading a config file at run time for something per launch: use a flag
  or a variable (`--port`, `AIORT_DATA`); `aio.toml` is built in.
- Adding features one by one per app: each other set is one more build of
  aio-rs in the shared build folder. Keep a template's set unless the app
  needs more.
- Building with plain `cargo build --release` to measure size: `ax build
  --sizes` uses ax's link flags; plain cargo is some percent bigger.

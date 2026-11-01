# Examples

Whole small apps, each on one page: the cells, the views, the config,
the launcher, the tests and the `ax` commands that run and inspect it.
Each page builds one app from nothing, step by step, and every Rust block
on it is a test of aio-rs: it compiles, and its logic runs, every time
aio-rs's tests run. What you read is what works with this version.

| Example | The app | What it shows |
|---|---|---|
| [A dashboard](dashboard.md) | live machine metrics, served to every screen | a schedule, an async method, a `#[table]` history, users by token, `visible`, a route, an SVG chart |
| [A checkout workflow](workflow.md) | a cart, a stock and a multi-step payment | a state machine, effects that call back, refusals as the error path, two cells working together |
| [A CLI and a service](cli-service.md) | a job queue and the tool that drives it | a headless server, a scheduled worker, crash recovery, a `client`-mode CLI, `aio::cli`, pairing |
| [A desktop app](desktop-app.md) | a local notes app | pages and Back, a menu, shortcuts, open and save dialogs, the tray, later launches, an update banner |
| [Integrations](integrations.md) | a shop hub | an outside HTTP API with backoff, a signed webhook, streamed uploads, OIDC sign-in, monitoring |

## Which one to read

- New to aio-rs: the [tutorial](../basics/tutorial.md) first, then the
  [desktop app](desktop-app.md): one cell, its window, its files.
- Several devices or people on the same data: the
  [dashboard](dashboard.md).
- A flow with steps that can fail, or must not run twice: the
  [checkout workflow](workflow.md).
- No window at all: the [CLI and service](cli-service.md).
- Anything that crosses the app's edge (HTTP in or out, sign-in,
  monitoring): [integrations](integrations.md).

## Concepts by example

| Concept | Where |
|---|---|
| a cell, its methods, refusals | every page |
| `persist`, `coalesce` | [desktop app](desktop-app.md#1-the-cell) |
| `shared`, `#[client]`, sync | [dashboard](dashboard.md), [CLI and service](cli-service.md) |
| `visible`, roles | [dashboard](dashboard.md#1-the-metrics-cell) |
| `machine = field` | [workflow](workflow.md#2-the-checkout-and-its-machine) |
| `#[derived]` | [dashboard](dashboard.md), [workflow](workflow.md), [desktop app](desktop-app.md) |
| async methods | [dashboard](dashboard.md#1-the-metrics-cell), [integrations](integrations.md#1-calling-an-http-api) |
| explicit effects, `cx.abort` | [workflow](workflow.md#3-paying-the-effect), [CLI and service](cli-service.md), [desktop app](desktop-app.md) |
| `#[every]`, `cx.after`, `cx.backoff` | [dashboard](dashboard.md), [CLI and service](cli-service.md), [integrations](integrations.md) |
| `restore` after a crash | [CLI and service](cli-service.md#1-the-services-cell) |
| `#[table]` and `ax sql` | [dashboard](dashboard.md) |
| blobs | [integrations](integrations.md#2-webhooks) |
| routes, server HTTP | [dashboard](dashboard.md#2-who-connects-users-and-a-route), [integrations](integrations.md) |
| users by token, OIDC | [dashboard](dashboard.md#2-who-connects-users-and-a-route), [integrations](integrations.md#4-sign-in-with-an-identity-provider) |
| `aio::cli` | [CLI and service](cli-service.md#3-the-tool) |
| views, `part`, `act!` | [dashboard](dashboard.md#3-the-views), [workflow](workflow.md#4-the-window), [desktop app](desktop-app.md#2-the-window) |
| routing, the kit, dialogs, tray | [desktop app](desktop-app.md#2-the-window) |
| updates | [desktop app](desktop-app.md#4-config-data-and-shipping) |
| `TestCells`, `TestServer`, `TestUi` | every page |

## Running an example

Each page says which template to start from (`ax create <name>`, with
`--template service` or `--template cli` for the headless ones), which
aio-rs features its `Cargo.toml` needs, and what goes in `src/lib.rs`,
`aio.toml` and the launcher. Copy the blocks in order: the lines a block
repeats from the one before are hidden on the page, so a block shows
only what it adds.

To run the pages' code as the tests it is, from a checkout of aio-rs:

```sh
cargo test -p aio-rs --doc --all-features examples   # the four pages that need aio-rs only
cargo test -p aio-rs-shell --doc --all-features      # the desktop app, which calls the desktop host
```

A block marked `no_run` (one that opens a window, or a `main` that would
start a real app) is compiled but not run; its logic runs in the blocks
before it, and its window test runs headless when it is part of an app
(`ax test <app>`).

## Coming from aio

Each page ends with a table from aio's version of the same app to
aio-rs's, and a list of its limits: what aio does there that aio-rs does
otherwise or not at all. The construct-by-construct guide is
[migration](../migration/README.md).

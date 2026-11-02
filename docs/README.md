# aio-rs documentation

aio-rs is a Rust framework for apps whose state is cells with an event
log: every change is saved, replayable, inspectable and synced; one
codebase runs as a desktop window, in a browser, on Android, as a server
or as a command-line tool. Its command is `ax`. This is the guide for
aio-rs 0.2.0 (alpha). Every Rust example in it compiles and runs as a
test of aio-rs, and every `ax` command shown exists with those flags.
Pages marked are being written.

Coming from aio, or choosing between them: [aio and aio-rs compared](aio-vs-aio-rs.md).

New here? Read in this order: [setup](basics/setup.md) →
[quickstart](basics/quickstart.md) → [tutorial](basics/tutorial.md) →
[concepts](basics/concepts.md), then the guide for what you build.

## 1. Get started

- [When to use aio-rs, and when not](basics/when.md): read first. Window and state yes; SEO, CDN, edge no; pin the tag; what is experimental.
- [Why aio-rs, not plain Rust](basics/why.md): what it gives over a UI crate, SQLite and sync by hand, and when not to use it.
- [Install and set up](basics/setup.md): what each OS needs, installing `ax` prebuilt or from source, `ax setup`, uninstalling.
- [Quickstart](basics/quickstart.md): from nothing to a running, tested, packaged app; what the template is made of.
- [Tutorial](basics/tutorial.md): a todo app built step by step: cell, invariant, tests, schedule, window, shipping.
- [Concepts](basics/concepts.md): cells, methods, effects, views, the log, doors, the crates.
- [Architecture](basics/architecture.md): how an app is put together, where each part of its code runs, the shapes an app can take.
- [Project layout, `aio.toml` and features](basics/project.md): the files of an app, its launcher, its Cargo features.
- [Cookbook](basics/cookbook.md): short, complete recipes for common tasks.
- [Versions and stability](basics/versioning.md): what aio-rs 0.x promises, how an app's versions are numbered.

## 2. Guides

**State**
- [Cells and methods](state/cells.md): a cell, its flags, derived values, invariants, state machines, the gate.
- [Methods in depth](state/methods.md): arguments, results and refusals, `cx`, atomic calls, transactional work across cells.
- [Lifecycle](state/lifecycle.md): start, stop, enable, disable, the breaker, shutdown, owned resources.
- [Composition](state/composition.md): many cells in one app, dependencies between them, shared types.
- [Effects, async methods and notifications](state/effects.md): effects and their modes, retries, async methods, telling people, reactions.
- [Schedules, real-time and heavy work](state/scheduling.md): `#[every]`, `#[cron]`, timers, real-time updates, work off the cell's thread.
- [Sync, clients and access](state/sync.md): servers and copies, offline, `#[client]` rules, server functions, routes, pairing, discovery, peers.
- [Visibility and contexts](state/visibility.md): what each viewer sees (`visible`), per-user views, contexts a method runs in.
- [Plugins and other apps](state/plugins.md): plugins, calling other aio-rs and aio apps.

**Data**
- [Persistence](persistence/README.md): the store and its log, coalesced writes, durability, migrations.
- [Files, backups and restore](persistence/files.md): where an app's files live, profiles and instances, backups.
- [Tables, SQL and blobs](persistence/sql.md): `#[table]` fields, raw SQL, live queries, blobs, big data.
- [Offline and merging](persistence/offline.md): working offline, merging edits, what travels on the wire (patches).

**UI**
- [Views](ui/views.md): `client!`, `part`, `act!`, components, local state, the page, the window.
- [The UI kit](ui/kit.md): controls, keys, long lists, dialogs and toasts.
- [Forms](ui/forms.md): fields, validation, submitting, errors.
- [Routing](ui/routing.md): `routes!`, `router!`, nested routes, deep links, Back.
- [Theming and styles](ui/theming.md): CSS and live CSS, the hue, tokens, dark mode, a CSS toolchain.
- [Motion and animation](ui/animation.md): transitions, springs, moving rows.
- [Islands and advanced views](ui/islands.md): other sites and foreign widgets in a view, portals, advanced patterns.
- [Performance](performance.md): fine-grained reads, coalesced writes, memoizing, windowed lists; measuring.

**Testing**
- [Testing](testing/README.md): which harness for what, the sandbox every test runs in, test lanes.
- [Testing cells](testing/cells.md): `TestCells`, refusals, fuzzing, time, effects.
- [Testing views](testing/ui.md): `TestUi`: the real views headless, clicks, keys, text, canvas.
- [Cassettes](testing/cassettes.md): recording and replaying I/O.
- [Servers, sync and many apps](testing/multi-app.md): `TestServer`, sync simulation, tests close to production.
- [Lint and proving the tests](testing/lint.md): `ax lint`, what it catches, tests that test something.
- [Proving the tests](testing/proving.md): the mutation ledger, breaks in the source and in a built `.deb`.
- [Real machines](testing/labs.md): `ax lab`: the app on a real Windows, macOS or Linux desktop.

**Debugging**
- [Debugging](debugging/README.md): from an error to a fix: the running app, logs, reports, health.
- [Errors](debugging/errors.md): every refusal and error code, what it means, what to do.
- [Time travel and replay](debugging/time-travel.md): `ax timetravel`, `ax replay`, stepping through the log.
- [Production and vitals](debugging/production.md): health, vitals, memory, budgets, a production checklist.
- [Feedback and problem reports](debugging/feedback.md): what a person sends, what a report holds.

**Security**
- [Accounts, keys and security](auth/README.md): the key, accounts, TOTP, OIDC, TLS, security headers, capabilities.
- [Secrets and observability](auth/secrets.md): secrets in state and files, redaction, what logs never hold.

**Shipping**
- [Develop, build and install](build/README.md): `ax dev`, `ax build`, installing.
- [Development mode](build/dev.md): hot-patching, live CSS, displays, running from source, imports.
- [Targets](build/targets.md): desktop, web, Android, a service, a command-line tool, and how they differ.
- [Release and update](build/release.md): keys, `ax release`, channels, delta updates, the data gate, rollback, signing.
- [CI for an app](build/ci.md): the GitHub workflow, what to run, a release check list.
- [Scale and benchmarks](build/scaling.md): how far one app goes, limits, measured numbers.

**Clients and tools**
- [Command-line apps](clients/cli-apps.md): `aio::cli`: arguments, prompts, progress.
- [The desktop window](clients/desktop.md): the shared Electron runtime, windows, menus, tray, background jobs.
- [In a browser](clients/web.md): the web build, a browser tab, embedded web views.
- [Transports and streams](clients/transports.md): how clients reach the app: sockets, IPC, HTTP, binary data.

**Examples**
- [Examples](examples/README.md): whole apps, each built and tested.
- [A dashboard](examples/dashboard.md): live data, charts, a server.
- [A checkout workflow](examples/workflow.md): a state machine, async steps, refusals.
- [A CLI and a service](examples/cli-service.md): a command-line tool talking to a headless service.
- [A desktop app](examples/desktop-app.md): a window app with menus, files and updates.
- [Integrations](examples/integrations.md): calling outside services: HTTP, OIDC, webhooks.

## 3. Reference

- [`aio.toml` reference](reference/aio-toml.md): every section and key, with its default.
- [Environment variables](reference/environment.md): every `AIORT_*` and `AIO_*` variable an app or `ax` reads.
- [ax command reference](clients/ax.md): every command and flag.
- [API map](reference/api.md): the public modules and items of each crate, with links into cargo doc.
- API docs: `cargo doc -p aio-rs --no-deps --open` (the core, `use aio::…`); `-p aio-rs-shell` for the desktop host.
- [aio-rs for coding agents](agents.md): `ax agent` and its rules.
- [aio-rs next to other Rust app stacks](comparison.md): one app built in each, measured.
- [CHANGELOG](../CHANGELOG.md): what each version ships, and its known limits.

## 4. Coming from aio

aio is the Deno-based framework (command `am`) aio-rs is the Rust
counterpart of.

- [aio and aio-rs compared](aio-vs-aio-rs.md): measured, compatibility, deviations, the feature matrix.
- [Migrating from aio to aio-rs](migration/README.md): construct-by-construct tables, a worked example, a checklist.
- [aio app data](migration/data.md): the `state.db` format both read and write.
- [aio wire formats](migration/wire.md): protocol v3, pairing, discovery, peers.

## 5. Help

- [FAQ and pitfalls](basics/faq.md): questions people ask, mistakes people make.
- [Troubleshooting](debugging/troubleshooting.md): symptom, cause and fix, per OS.
- `ax setup --check` names each missing tool and the command that gets it; `ax doctor <app>` checks a running app against its code.

## aio's docs, page by page

Where each page of aio's documentation is covered here.

| aio | aio-rs |
|---|---|
| `AGENTS.md` | [aio-rs for coding agents](agents.md) |
| `ai.md` | [aio-rs for coding agents](agents.md) |
| `auth/README.md` | [Accounts, keys and security](auth/README.md) |
| `auth/auth.md` | [Accounts, keys and security](auth/README.md) |
| `auth/secrets-and-observability.md` | [Secrets and observability](auth/secrets.md) |
| `basics/README.md` | [Install and set up](basics/setup.md) |
| `basics/api-reference.md` | [API map](reference/api.md) |
| `basics/app-architectures.md` | [Architecture](basics/architecture.md) |
| `basics/architecture.md` | [Architecture](basics/architecture.md) |
| `basics/concepts.md` | [Concepts](basics/concepts.md) |
| `basics/cookbook.md` | [Cookbook](basics/cookbook.md) |
| `basics/cross-app.md` | [Plugins and other apps](state/plugins.md) |
| `basics/every-option.md` | [`aio.toml` reference](reference/aio-toml.md) |
| `basics/faq.md` | [FAQ and pitfalls](basics/faq.md) |
| `basics/index.md` | [Install and set up](basics/setup.md) |
| `basics/migration.md` | [Migrating from aio to aio-rs](migration/README.md) |
| `basics/pitfalls.md` | [FAQ and pitfalls](basics/faq.md) |
| `basics/plugins.md` | [Plugins and other apps](state/plugins.md) |
| `basics/positioning.md` | [aio-rs next to other Rust app stacks](comparison.md) |
| `basics/project-structure.md` | [Project layout, `aio.toml` and features](basics/project.md) |
| `basics/quickstart.md` | [Quickstart](basics/quickstart.md) |
| `basics/semver-policy.md` | [Versions and stability](basics/versioning.md) |
| `basics/tutorial.md` | [Tutorial](basics/tutorial.md) |
| `basics/where-code-runs.md` | [Architecture](basics/architecture.md) |
| `basics/windows-macos.md` | [Install and set up](basics/setup.md) |
| `basics/windows-macos.md (problems)` | [Troubleshooting](debugging/troubleshooting.md) |
| `build/README.md` | [Develop, build and install](build/README.md) |
| `build/benchmarks.md` | [Scale and benchmarks](build/scaling.md) |
| `build/ci.md` | [CI for an app](build/ci.md) |
| `build/dev-mode.md` | [Development mode](build/dev.md) |
| `build/environment.md` | [Environment variables](reference/environment.md) |
| `build/imports.md` | [Development mode](build/dev.md) |
| `build/run-from-source.md` | [Development mode](build/dev.md) |
| `build/runtime-differences.md` | [Targets](build/targets.md) |
| `build/scaling.md` | [Scale and benchmarks](build/scaling.md) |
| `build/targets.md` | [Targets](build/targets.md) |
| `build/validation-runbook.md` | [CI for an app](build/ci.md) |
| `build/versioning.md` | [Versions and stability](basics/versioning.md) |
| `clients/README.md` | [ax command reference](clients/ax.md) |
| `clients/app-manager.md` | [ax command reference](clients/ax.md) |
| `clients/binary-streams.md` | [Transports and streams](clients/transports.md) |
| `clients/browser.md` | [In a browser](clients/web.md) |
| `clients/cli-toolkit.md` | [Command-line apps](clients/cli-apps.md) |
| `clients/desktop-jobs.md` | [The desktop window](clients/desktop.md) |
| `clients/electron.md` | [The desktop window](clients/desktop.md) |
| `clients/notifications.md` | [Effects, async methods and notifications](state/effects.md) |
| `clients/transports.md` | [Transports and streams](clients/transports.md) |
| `clients/transports.md (protocol)` | [aio wire formats](migration/wire.md) |
| `clients/webview.md` | [In a browser](clients/web.md) |
| `debugging/README.md` | [Debugging](debugging/README.md) |
| `debugging/errors.md` | [Errors](debugging/errors.md) |
| `debugging/feedback.md` | [Feedback and problem reports](debugging/feedback.md) |
| `debugging/performance.md` | [Performance](performance.md) |
| `debugging/production.md` | [Production and vitals](debugging/production.md) |
| `debugging/time-travel.md` | [Time travel and replay](debugging/time-travel.md) |
| `debugging/troubleshooting.md` | [Troubleshooting](debugging/troubleshooting.md) |
| `debugging/vitals.md` | [Production and vitals](debugging/production.md) |
| `deploy/signing.md` | [Release and update](build/release.md) |
| `deploy/updates.md` | [Release and update](build/release.md) |
| `examples/` | [Examples](examples/README.md) |
| `examples/01-dashboard.md` | [A dashboard](examples/dashboard.md) |
| `examples/02-checkout-workflow.md` | [A checkout workflow](examples/workflow.md) |
| `examples/03-cli-service.md` | [A CLI and a service](examples/cli-service.md) |
| `examples/04-electron-app.md` | [A desktop app](examples/desktop-app.md) |
| `examples/05-integrations.md` | [Integrations](examples/integrations.md) |
| `persistence/README.md` | [Persistence](persistence/README.md) |
| `persistence/auto-persist.md` | [Persistence](persistence/README.md) |
| `persistence/big-data.md` | [Tables, SQL and blobs](persistence/sql.md) |
| `persistence/crdt-protocol.md` | [Offline and merging](persistence/offline.md) |
| `persistence/crdt.md` | [Offline and merging](persistence/offline.md) |
| `persistence/delta.md` | [Offline and merging](persistence/offline.md) |
| `persistence/how-it-works.md` | [Persistence](persistence/README.md) |
| `persistence/how-it-works.md (format)` | [aio app data](migration/data.md) |
| `persistence/offline.md` | [Offline and merging](persistence/offline.md) |
| `persistence/sqlite.md` | [Tables, SQL and blobs](persistence/sql.md) |
| `persistence/where-files-live.md` | [Files, backups and restore](persistence/files.md) |
| `state/README.md` | [Cells and methods](state/cells.md) |
| `state/cell-contexts.md` | [Visibility and contexts](state/visibility.md) |
| `state/cell-visibility.md` | [Visibility and contexts](state/visibility.md) |
| `state/cell-workers.md` | [Schedules, real-time and heavy work](state/scheduling.md) |
| `state/cells.md` | [Cells and methods](state/cells.md) |
| `state/composition.md` | [Composition](state/composition.md) |
| `state/lifecycle.md` | [Lifecycle](state/lifecycle.md) |
| `state/methods.md` | [Methods in depth](state/methods.md) |
| `state/real-time.md` | [Schedules, real-time and heavy work](state/scheduling.md) |
| `state/scheduling.md` | [Schedules, real-time and heavy work](state/scheduling.md) |
| `state/scheduling.md (effects `part`)` | [Effects, async methods and notifications](state/effects.md) |
| `state/the-bridge.md` | [Concepts](basics/concepts.md) |
| `state/the-bridge.md (sync `part`)` | [Sync, clients and access](state/sync.md) |
| `state/transactional-methods.md` | [Methods in depth](state/methods.md) |
| `testing/README.md` | [Testing](testing/README.md) |
| `testing/canvas-and-3d.md` | [Testing views](testing/ui.md) |
| `testing/cassettes.md` | [Cassettes](testing/cassettes.md) |
| `testing/cell-testing.md` | [Testing cells](testing/cells.md) |
| `testing/lanes.md` | [Testing](testing/README.md) |
| `testing/linter.md` | [Lint and proving the tests](testing/lint.md) |
| `testing/multi-app.md` | [Servers, sync and many apps](testing/multi-app.md) |
| `testing/onboarding-lab.md` | [Real machines](testing/labs.md) |
| `testing/prod-parity.md` | [Servers, sync and many apps](testing/multi-app.md) |
| `testing/proving-the-tests.md` | [Proving the tests](testing/proving.md), [Lint](testing/lint.md) |
| `testing/ui-testing.md` | [Testing views](testing/ui.md) |
| `testing/vm-labs.md` | [Real machines](testing/labs.md) |
| `ui/README.md` | [Views](ui/views.md) |
| `ui/air-advanced.md` | [Islands and advanced views](ui/islands.md) |
| `ui/air-animation.md` | [Motion and animation](ui/animation.md) |
| `ui/air-comparison.md` | [Views](ui/views.md) |
| `ui/air-components.md` | [Views](ui/views.md) |
| `ui/air-forms.md` | [Forms](ui/forms.md) |
| `ui/air-lifecycle.md` | [Views](ui/views.md) |
| `ui/air-reference.md` | [Views](ui/views.md) |
| `ui/air-routing.md` | [Routing](ui/routing.md) |
| `ui/air-setup.md` | [Views](ui/views.md) |
| `ui/air-signals.md` | [Views](ui/views.md) |
| `ui/comparison.md` | [Views](ui/views.md) |
| `ui/css-toolchain.md` | [Theming and styles](ui/theming.md) |
| `ui/kit.md` | [The UI kit](ui/kit.md) |
| `ui/react-islands.md` | [Islands and advanced views](ui/islands.md) |
| `ui/react.md` | [Views](ui/views.md) |
| `ui/reactivity-tracking.md` | [Performance](performance.md) |
| `ui/theme.md` | [Theming and styles](ui/theming.md) |

Working on aio-rs itself (building it, its gates, releasing it):
[CONTRIBUTING.md](../CONTRIBUTING.md).

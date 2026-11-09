# Lint and proving the tests

`ax lint [dir]` checks what Rust's types and aio-rs's macros can't: a
credential every client gets, a method that won't replay, a hook that
runs only sometimes, markup a screen reader can't name, two cells on one
store row. Then it runs clippy with aio-rs's lints. It reads the source;
no app has to run.

Much of what aio's linter (`aiol`) checks is a compile error in aio-rs:
a cell is a typed struct, a method takes `&mut self` and returns
`aio::Result`, imports and the client/server split are resolved by
`rustc`, cells reach each other only through typed handles, and an
effect is `#[must_use]`. `ax lint` covers the rest.

## Running it

```sh
ax lint                  # the project here (the nearest Cargo.toml at or above)
ax lint apps/notes       # another project
ax lint --no-clippy      # aio-rs's own checks only: fast, nothing is built
ax lint --no-hints       # hint lines left out (still counted)
ax lint --json           # one JSON document on stdout, hints always in
ax lint --safe-fix       # clippy's machine-applicable fixes first, then lint what is left
ax lint --fix            # move an app of an older layout to today's, then lint
```

| Flag | Does |
|---|---|
| `dir` | the project: the nearest `Cargo.toml` at or above it (default: here) |
| `--json` | one JSON document on stdout; narration goes to stderr |
| `--no-hints`, `--quiet-hints` | hint lines left out of the report, still counted in the summary |
| `--no-clippy` | only aio-rs's checks: no build, about a second |
| `--safe-fix` | `cargo clippy --fix` (only machine-applicable fixes; denied lints only warn while fixing), then the lint |
| `--fix` | moves an app of an older layout to `ax create`'s: no launcher `build.rs`, no aio-rs build-dependency, no `video` feature, renamed paths rewritten; then the lint |

Exit: 0 with no errors (warnings and hints don't fail), 1 with errors,
2 for bad usage.

## The report

```text
ax lint 0.1.0 · notes

  ✓ 1 cell(s): Notes
  ✗ error src/lib.rs:7  [security] cell `Notes` field `api_key` is named like a credential, and every client of the cell gets it
      fix: mark it #[server] (kept and saved, never sent) or filter it with #[cell(visible = f)]; if it really is public, say why: // aio-ok(security): <why>
  ✗ error src/lib.rs:14  [cells] `Notes::add` uses the clock (`std::time::SystemTime::now`): methods replay from the log, so they must give the same result every time
      fix: use cx.now(): the time of the call, logged with it
  ✗ error src/lib.rs:22  [hooks] `signal` in `List` runs only sometimes (in a branch, a loop or a closure, or after an early `return` or `?`): hooks are matched by call order, so a run of the body that skips one hands the next hook its state
      fix: call it at the top of the function, on every run, before any early `return` or `?`, and branch on its value
  ✗ error src/lib.rs:25  [a11y] <input> has no label: screen readers and agents can't name it
      fix: aria_label: "…", or wrap it in label { }
  ⚠ warn src/lib.rs:26  [a11y] <div> takes clicks but no keys: keyboard users and agents can't use it
      fix: use a button, or give it role, tabindex: 0 and onkeydown
  ✗ error src/lib.rs:28  [render] <li> in a for loop has no key: a removed or moved row would hand its state to the next (air refuses it)
      fix: key: "{item.id}"
  · hint src/lib.rs:5  [testing] no test names cell `Notes`
      fix: test its methods with Handle::<Cell>::open on a temp store, or aio::testing

  Files: 2  Cells: 1  Tests: 0
  4 errors · 2 warnings · 1 hint
```

| Level | Mark | Means |
|---|---|---|
| error | `✗` | breaks at run time, leaks, or won't replay: fix it (exit 1) |
| warn | `⚠` | likely a bug: fix it or say why not |
| hint | `·` | works, could be better |

`✓` lines are what passed: the cells found, each `aio.toml` that parses,
"all N cells are tested".

A finding a flag fixes ends in `[fixable: --safe-fix]` (clippy has a
machine-applicable fix) or `[fixable: --fix]` (the older layout), aio's
`[fixable]`, and the summary counts them under the command:

```text
  ⚠ warn src/lib.rs:3  [rustc] unused import: `std::fmt`  [fixable: --safe-fix]
      fix: see unused_imports
  …
  1 finding fixable: ax lint --safe-fix
```

### JSON

`--json` prints aio's shape, so CI tools written for `aiol` read it:

```json
{
  "version": "0.1.0",
  "stats": { "filesScanned": 2, "cellsFound": 1, "testsFound": 0 },
  "passed": ["1 cell(s): Notes"],
  "issues": [
    {
      "severity": "error",
      "area": "cells",
      "message": "`Notes::add` uses the clock (`std::time::SystemTime::now`): methods replay from the log, so they must give the same result every time",
      "file": "src/lib.rs",
      "line": 14,
      "fix": "use cx.now(): the time of the call, logged with it"
    }
  ],
  "summary": { "errors": 4, "warnings": 2, "hints": 1 }
}
```

`file`, `line`, `fix` and `fixable` (`"--safe-fix"`, `"--fix"`) are left
out when a finding has none.

## What it checks

Each finding has an area, in brackets in the report.

### `security`

| Finding | Level |
|---|---|
| a `#[cell]` field named like a credential (`password`, `passphrase`, `mnemonic`, `api_key`, `private_key`, `secret_key`, `access_token`, `auth_token`, in any case style) that every client of the cell gets | error |
| a well-known key in the source or any text file: an AWS access key, GitHub, GitLab or Slack tokens, a Google API key, Stripe live keys, `sk-…` secret keys, a PEM private key | error |
| a value assigned to a credential-like name that looks real (long enough, no placeholder such as `example`, `changeme`, `xxxx`, no `{`, `$`, `<`) | warn |
| a `.env` file git would commit | warn |

- Names are split into words, never substrings: `monkey` has no `key`,
  `apiKey`, `API_KEY` and `api_key` are the same name.
- A field marked `#[server]` (kept and saved, never sent) or a cell with
  `#[cell(visible = f)]` (each viewer gets what `f` gives) is fine
  ([Visibility](../state/visibility.md)).
- Test code (`tests/`, `benches/`, `tests.rs`, `*_test.rs`) isn't
  scanned for values, so a test may hold a fake key.
- A hardcoded key: revoke it, then load the new one at run time
  ([Secrets](../auth/secrets.md)).

### `cells`

| Finding | Level |
|---|---|
| a method that reads the clock (`SystemTime::now`, `Instant::now`, chrono's `Utc::now`/`Local::now`, `OffsetDateTime::now_utc`) | error: use `cx.now()` |
| a method that draws randomness (`rand`, `thread_rng`, `OsRng`, `getrandom`, `Uuid::new_v4`, `aio::uuid_v4`, `random_bytes`) | error: use `cx.rand()` or `cx.id()` |
| a method that reads the environment (`env::var`, `args`, `current_dir`) | error: pass it in as an argument |
| a method that does file, terminal, network or process I/O, or starts a thread or a task | error: do it in an effect |
| two cells with one id (`#[cell(id = "…")]`, else the type's name in snake case) | error: they would share one store row |

A method runs again on replay, sync and time travel: whatever it reads
from the world must come from the call ([Methods do no
I/O](../state/cells.md#methods-do-no-io)).

### `perf`

| Finding | Level |
|---|---|
| blocking std I/O in async code (an `async fn`, an `async` block or closure): `std::fs`, `File`, `OpenOptions`, `std::net` sockets, `thread::sleep`, `ureq`, `reqwest::blocking`, by full path or through a `use std::…` | warn: use the async one (`tokio::fs`, `tokio::time::sleep`), or `aio::effect::blocking(…)` |
| a crate (a folder with a `Cargo.toml`) of more than 15 000 lines of Rust in its `src`, tests left out | hint, at its `Cargo.toml`: `ax dev` recompiles all of it for an edit anywhere in it (~1 s per 15 000 lines); keep its views in page crates (`ax add <app> page <name>`) |

aio's sync-I/O hint: an effect that blocks holds the runtime thread, so
every effect and call behind it waits. A plain closure's body is left
alone (`spawn_blocking(|| …)` is where blocking work goes). aio's other
perf hints are measured here instead: render cost with
`TestUi::components` and `ax components`, a method's time with `[perf]`
budgets.

### `hooks`

| Finding | Level |
|---|---|
| a hook (`use_*`, or `aio::ui`'s `signal`, `memo`, `effect`, `watch`, `once`, `on_cleanup`, `callback`, `provide`, `task`, `resource`) in a component or a `use_` function called in a branch, a loop, a closure, or after an early `return` or `?` | error |
| `Signal::new(..)` (and the like) in a component's body: a new signal each time the body runs (again once a resource loads, after a hot reload), its value lost | error: `signal(\|\| ..)` |

### `render`

| Finding | Level |
|---|---|
| a `for` loop's element or component with no `key` (air refuses it) | error |
| a `for` loop over a block (its `rsx!` inside an unkeyed node) | warn |
| a `key` on a node that isn't its block's first, or on a node in no list | warn |
| a `.read()`, `.read_untracked()` or `.write()` guard held across an `if let`, `while let` or `match` whose block writes: it panics as already borrowed | warn |
| page CSS made in `once`: an edit never shows under `ax dev` | error |
| `ondragstart` that calls `set_data`: the view runs after the browser's drag start, so the drop never gets it | warn: `"data-drag": "{id}"` |

### `renderer`

| Finding | Level |
|---|---|
| a path to aio-rs 0.1's renderer crates in the app's Rust (a `use` of them, tests too): views name `aio::ui` only | error |
| a page script that talks to aio-rs 0.1's renderer's page object (its `send`/`recv`): the page object is `aio` (`aio::ui::eval`) | error |
| a `Cargo.toml` dependency on aio-rs 0.1's renderer crates: `aio` with feature `kit` brings air | error |

### `a11y`

| Finding | Level |
|---|---|
| an `<img>` with no `alt`, a `<button>` with no name, an `<input>`, `<select>` or `<textarea>` with no label | error |
| an element that takes clicks but no keys (not focusable, no `onkeydown`) | warn |
| a `<button>` in a form with no `type` (it submits), an `<a>` with `onclick` and no `href` | warn |
| a positive `tabindex`, an `aria-hidden` element that is focusable, `aria-disabled` with a live `onclick` | warn |

These are also what `TestUi` needs: an element a screen reader can't
name has no [auto name](ui.md#auto-names) either.

### The rest

| Area | Finding | Level |
|---|---|---|
| `testing` | a cell no test names (its type isn't mentioned in any test) | hint |
| `testing` | tests that use `aio::testing` with no manifest giving them aio-rs's `testing` feature (its own dependency, or `workspace = true` on a `[workspace.dependencies]` aio-rs, renamed or not) | error |
| `testing` | a `#[test]` that checks nothing: no assertion, `?`, `unwrap`/`expect` or harness wait in it (`no-assertions`; `#[should_panic]` is a check) | warn |
| `testing` | `assert_eq!(x, x)`, `assert_ne!(x, x)`: a thing compared with itself (`self-comparison`) | warn |
| `css` | a class two of the app's stylesheets define at their top level (`.track { … }`) with one property set to two values: whichever loads last wins, silently (aio's `aiol` styles check; `:hover` and longer selectors, rules inside `@media`, custom properties and a generated sheet starting `/*!` are left alone) | warn |
| `config` | an `aio.toml` that doesn't parse or check | error |
| `migrate` | an older layout: a launcher `build.rs`, `aio::config!()?`, renamed paths, an aio-rs build-dependency, the `video` feature (`testing` brings it) | warn or error (`--fix` moves it) |
| `scan` | a `.rs` file that doesn't parse, a text file over 1 MiB (not checked) | warn, hint |
| `clippy`, `rustc` | the project's own clippy and compiler warnings, plus `dbg!` (warn), `todo!()` and `unimplemented!()` (error) | as reported |

## Silencing one finding

```rust
use aio::prelude::*;

#[cell]
pub struct Demo {
    // aio-ok(security): a public demo key, shown on the page on purpose
    pub api_key: String,
}

#[methods]
impl Demo {}

# fn main() {}
```

`// aio-ok: <why>`, or `// aio-ok(<area>): <why>` to name the area
(`aio-ok(security,a11y): …` for two), on the line or in the comment block
right above it (a blank line ends the block). The reason is required: a
marker without one silences nothing. A reviewer can read the claim; a
lint setting can't be read where the code is.

## In CI

```sh
ax lint --no-clippy && cargo clippy --all-targets -- -D warnings && ax test notes
```

`ax lint` exits 1 on errors only. A CI job that should also fail on
warnings reads `--json` (`summary.warnings`). Running clippy yourself
with your own flags, `--no-clippy` keeps `ax lint` fast.

## Proving the tests

A green suite says the tests passed, not that they would have noticed a
bug. aio-rs's harnesses are built so that a test can't pass by accident:

| A test that passes without testing | Caught by |
|---|---|
| a refusal or failure nobody looked at | `TestCells`, `TestUi`: failing at the test's end unless taken (`failures()`) |
| "it threw" for the wrong reason | refusals are `Rejected` values: compare `why`, not `is_err()` |
| waiting that gave up quietly | `settle`, `until`, `converged` fail, naming what still runs or lags |
| a seed that pins nothing | `seeded` refuses a key the state lacks |
| a lookup that matched the wrong element | `text=` and `css=` refuse two matches; a miss lists what is there |
| an action a person couldn't do | hidden, disabled and covered elements refuse clicks and keys |
| a random test that can't be re-run | `fuzz` and `Sim` are seeded and print the seed |
| the real network reached by accident | `TestUi` refuses it; a cassette never calls through on replay |
| a cell nobody tests | `ax lint`'s `testing` hint |
| a test that checks nothing, a value compared with itself | `ax lint`'s `testing` warnings |

aio's three gates on its own tests, and aio-rs's:

| aio | aio-rs |
|---|---|
| `check:mutations`: a ledger of invariants, each broken on purpose, its named test required to go red | `scripts/mutants` in the aio-rs repository, in the source and in a built `.deb` ([Proving the tests](proving.md)); for an app, `ax test <app> --mutants` (`cargo mutants`) |
| `check:vacuous`: tests with no assertion, assertions in an empty loop, `length > 0`, a value compared with itself, a swallowing `try` | `ax lint`: no assertion, self-comparison; review for the rest (below) |
| `check:dead-wiring`: exports nothing in `src/` reaches | rustc's `dead_code` warnings for private items; `pub` items unused by the app aren't flagged |

The shapes to look for in review, and their fix:

- **An assertion in a loop over a collection that may be empty.** Assert
  the length first (`assert_eq!(rows.len(), 3)`), then the rows.
- **`assert!(!msg.is_empty())`.** True of any message. Assert what it
  says (`assert!(msg.contains("no url"))`).
- **A value compared with itself** (`assert_eq!(f(x), f(x))`). Compare
  with a known answer.
- **A test that calls a helper instead of the path the app takes.** Call
  the method, click the button, `fetch` the route.
- **An `if let` around the assertion.** When the `if` doesn't match,
  nothing is asserted: `let Some(x) = .. else { panic!(..) }` instead.

The rule behind all of these: a test earns its green by being able to
go red. Break the line it covers once, by hand, and watch it fail.

## What aio has that aio-rs does without

| aio | aio-rs |
|---|---|
| `aiol` checks of types, imports, the client/server split, state shapes, effects dropped | compile errors: nothing to lint |
| perf hints: `useAio()` over direct reads | render cost measured with `TestUi::components` and `ax components` (sync I/O: the `perf` warning) |
| `--safe-fix` of `deno.json` config, imports and removed spellings | `--safe-fix` applies clippy's fixes; `--fix` moves an older layout |

## API

`ax lint --help`; the checks live in the `ax` crate (`aio-rs-cli`,
`lint`).

# CI for an app

What to run on every push, what to run on a schedule, and what to check
by hand before a release. aio-rs gives you two pieces: the commands
(`ax lint`, `ax test`, `ax build --smoke`), and a release workflow
(`ax release <app> --workflow`, [release](release.md#github-workflow)).
`ax create` writes no CI workflow of its own: the one below is the
starting point.

## What to run

| Step | Command | Catches |
|---|---|---|
| format | `cargo fmt --check` | unformatted code |
| lint | `ax lint` | what types can't check (credentials in shared state, impure methods, hooks, accessibility, cell ids) plus clippy; exit 1 on errors |
| tests | `ax test notes` | the cells' and the views' tests, UI tests headless in the real runtime |
| build | `ax build notes --smoke` | the release bundle, started headless, stopped cleanly, its log checked |
| size | `ax build notes --sizes` | the release binary against its budget (`[build] budget_mb`, 10 by default), and which crates take the space |
| docs | `cargo doc --no-deps` | broken links in your own API docs |

- `ax test` is `cargo test` with what aio-rs's tests need: UI tests draw
  in the Electron runtime the app's window runs, headless (no display);
  `ax setup` fetches that runtime. `AIORT_RUNTIME=<folder>` points the
  tests at a runtime fetched elsewhere.
- `ax build --smoke` starts the Linux bundles under a display of their
  own (Xvfb), so the runner needs `xvfb`; `[build] smoke = true` smokes
  every build.
- Every test runs in its own sandbox (its own data folder), so tests
  never touch the runner's home, and two jobs on one machine don't meet
  ([testing](../testing/README.md)).
- `ax lint --json` gives a CI's annotations one object per finding.

## The aio-rs dependency

`ax create` makes an app whose `aio-rs` crates are a **path** to the
checkout it was made from (`path = "/…/aio-rs/crates/aio"` in
`Cargo.toml` and `app/Cargo.toml`). A runner doesn't have that folder.
The release workflow (`ax release --workflow`) handles both cases by
itself ([release](release.md#github-workflow)); for this CI workflow,
two ways:

- **Depend by git** (the release workflow then installs `ax` from the
  same commit):

  ```toml
  aio = { package = "aio-rs", git = "https://github.com/riagentic/aio-rs", rev = "<commit>", features = ["kit"] }
  ```

  and the same `git`/`rev` for `aio-rs-shell` in `app/Cargo.toml`.
  `Cargo.lock` then pins the commit.
- **Check aio-rs out beside the app** and point the paths there, as the
  release workflow does: `actions/checkout` with `repository:
  riagentic/aio-rs`, `ref: v0.1.0`, `path: aio-rs`, the app's own checkout
  at `path: app`, then rewrite each `path = "…/aio"`, `"…/aio-shell"` to
  `$GITHUB_WORKSPACE/aio-rs/crates/…`.

## The workflow

`.github/workflows/ci.yml` in the app's repository:

```yaml
name: ci
on:
  push:
    branches: [main]
  pull_request:
permissions:
  contents: read
concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true
jobs:
  check:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0                # ax numbers builds from the commit count
      - run: sudo apt-get update && sudo apt-get install -y --no-install-recommends xvfb libgtk-3-0t64 libnss3 libgbm1 libasound2t64
      - run: rustup toolchain install   # the version rust-toolchain.toml pins
      - uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: cargo-${{ runner.os }}-${{ hashFiles('Cargo.lock') }}
      - name: Install ax
        run: cargo install --locked --git https://github.com/riagentic/aio-rs --rev <commit> aio-rs-cli
      - uses: actions/cache@v4
        with:
          path: ~/.local/share/aio/electron
          key: aio-runtime-${{ runner.os }}-<commit>
      - run: ax setup
      - run: cargo fmt --check
      - run: ax lint
      - run: ax test notes
      - run: ax build notes --smoke
```

Replace `<commit>` with the aio-rs commit the app's `Cargo.lock` pins.
For Windows and macOS add a matrix job that runs `ax test notes` on
`windows-2025` and `macos-15` (the runtime is fetched per OS: cache
`%LOCALAPPDATA%\aio\electron` and `~/Library/Application Support/aio/electron`).

`ax setup` exits 1 while something a build needs is missing and names
the command that gets it: on a runner it is the first thing to read when
a job fails early.

## Caching

| What | Where | Why |
|---|---|---|
| cargo's registry and `target/` | `actions/cache` keyed on `Cargo.lock` (or Swatinem's rust-cache action) | most of a build's time |
| the shared Electron runtime (~280 MB unpacked) | aio's Electron store, `aio/electron/` in the data home | `ax setup` fetches and checks it once |
| `ax` itself | `~/.cargo/bin/ax` (cache it keyed on the aio-rs commit) | `cargo install` builds it from source |

A cold job builds aio-rs, the app and `ax`; a warm one rebuilds only the
app.

## On a schedule, not every push

```yaml
on:
  schedule:
    - cron: "0 4 * * 1"
```

- `cargo audit` (or `cargo deny`): advisories for the dependencies
  appear without any change of yours.
- `ax build notes --platforms linux,windows,macos`: the cross builds
  (`--windows` needs `gcc-mingw-w64-x86-64` on Linux; a macOS bundle is
  built on a Mac).
- The tests against the newest aio-rs commit, to see a break before you
  move the pin.

## Before a release

The automated part, on the release commit:

- [ ] CI green: fmt, `ax lint`, `ax test`, `ax build --smoke`.
- [ ] `ax build <app> --sizes` within budget.
- [ ] `[app] version` raised where the data or the behaviour changed;
  a cell whose saved shape changed has its `version` and `migrate`
  ([persistence](../persistence/README.md)).
- [ ] `ax release <app> --channel beta` first: a copy on `beta` updates
  from the previous release, and its first start keeps the data.
- [ ] The signing key backed up, `AIO_SIGNING_KEY` set, the tag equals
  the version ([release](release.md#github-workflow)).

By hand, on a real desktop of each OS: `ax lab` builds the app for that
machine, copies it over ssh into `aio-rs-test/lab/<id>` and starts it on
the desktop of whoever is signed in there, its data, runtime and caches
inside that folder. The machines are `AIO_LAB_WINDOWS`, `AIO_LAB_MACOS`
and `AIO_LAB_LINUX` (`[ssh options] [user@]host`).

```sh
ax lab notes windows        # click around; then
ax lab notes windows --stop # what ran from it ended, the folder removed
```

| OS | Check |
|---|---|
| every desktop | it opens, the window looks right at its size, keys and menus work, it stops cleanly and starts again with its data |
| Windows | the one-click `.exe` installs for the user, SmartScreen's first-open path ([troubleshooting](../debugging/troubleshooting.md#windows)), the firewall prompt for a served app |
| macOS | the `.dmg` opens, the first open of a downloaded app, the offer to move into Applications, local-network access for pairing |
| Linux | the `.tar.gz`'s `install.sh`, the AppImage, the `.deb` |
| Android | `ax install notes --android` on a real phone: it starts offline, Back, deep links, a kill and restart keep the state |
| a served app | another machine reaches it (`--expose`), TLS and pairing, `ax health` |
| an update | an installed copy of the previous release takes the new one, and a broken one rolls back ([release](release.md#install-and-the-way-back)) |

Write down what you ran and saw, per OS, with the commit: a release you
can't say was checked wasn't.

## Not in aio-rs

- aio's `deno task check` / `deno task test` pair and their workflow
  template: aio-rs has `ax lint` and `ax test`, and no CI file from
  `ax create`.
- aio's lab boots throwaway containers and VMs; `ax lab` uses machines
  you already have.
- aio's one-command validation of the whole release matrix: the hand
  checks above stay a list.

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
| size | `ax build notes --sizes` | the release binary against its budget (`[build] budget_mb`, 10 by default, 15 on Windows), and which crates take the space |
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
| an update | an installed copy of the previous release takes the new one, and a broken one rolls back ([release](release.md#install-and-the-way-back)); aio-rs's own path is proven by `scripts/update-proof` (below), the app's is yours |

Write down what you ran and saw, per OS, with the commit: a release you
can't say was checked wasn't.

## aio-rs's own proof scripts

What aio-rs runs on itself before a release, in its checkout (each in a
throwaway home under `dist/`, removed after):

| Script | Proves | Where |
|---|---|---|
| `scripts/update-proof` | a sample service installed from its bundle (Linux `install.sh`, macOS `.app`, Windows `--install`) takes a signed release made from a patch, then a broken release fails its first boot and is rolled back, the data from before it read back | GitHub CI, Linux, macOS, Windows |
| `scripts/android-gate` | a sample APK starts on an emulator, and an upgrade installed over it keeps its data | before each release, on a machine with the Android SDK and KVM |
| `scripts/numbers` | the README's and docs' numbers, measured again, printed beside the published ones with the machine | any time (Linux x86_64 for the comparison) |

### The Android gate

A release needs `scripts/android-gate` green
([CONTRIBUTING](../../CONTRIBUTING.md#make-a-release)). It never touches
another emulator, a phone or the AVDs already on the machine:

- It makes its own AVD in its work folder (`ANDROID_AVD_HOME`), from the
  newest x86_64 system image in the SDK (`AIO_GATE_IMAGE` names another:
  `system-images;android-35;google_apis;x86_64`), and boots it with no
  window on console port 5600; every `adb` call names `-s emulator-5600`.
  An `emulator-5600` already attached stops it.
- `ax create gate`, with a saved cell that counts the app's starts and
  logs `[gate] start <n>`. `ax test gate --android --device emulator-5600`
  builds the APK for the emulator's CPU, installs it, starts it and
  passes when the window is ready and it still runs 3 s later: start 1.
- One commit on (a new version), the same over it (`adb install -r`, an
  upgrade): start 2, so the data of the first install was kept.
- After: the emulator stopped (`adb -s emulator-5600 emu kill`), the AVD
  and work folder removed, and `modem-nv-ram-5600`, which the emulator
  writes beside the user's AVDs, deleted.
- `--air`: the same with the template on air (aio-rs `air`, the shell
  with `default-features = false, features = ["air"]`): no Dioxus in its
  graph, and no `panicked` or failed view in the app's log.

Needs the SDK's `emulator`, `platform-tools`, `cmdline-tools/latest`, an
x86_64 system image, KVM, and what `ax build --android` needs
([setup](../basics/setup.md)).

### air's package gates

An app on air (aio-rs's own renderer, [the design](../design/air.md))
ships no Dioxus, and its packages and edit loop are measured, in
`ci.sh` and GitHub CI:

- `scripts/air-ship <app> [artifacts...]` fails, naming each find, on a
  Dioxus crate the app ships (its `cargo metadata` graph of normal
  dependencies for the desktop, wasm32 and Android: a crate by that name
  or from DioxusLabs' repositories, such as `generational-box`) and on the
  word in any package byte for byte, any case: the release binary, a
  folder (the web build), a `.tar.gz`, an AppImage (its squashfs
  unpacked), an APK (each member). A stripped binary still holds a
  crate's source paths and strings. `--self-test` (in `ci.sh`) plants
  one in each kind of artifact and in a graph, and requires each found.
- `scripts/dx-metrics` builds the template a second time on air (aio-rs
  `air`, the shell with `default-features = false, features = ["air"]`),
  plants a Dioxus dependency in it and requires air-ship to fail, then
  runs air-ship on its release binary, `.tar.gz`, AppImage and web
  build. Its row adds `air_binary_b`, `air_targz_b`, `air_appimage_b`,
  `air_web_b`: held 5% over the base measured on the same runner
  (`--base`, as the other sizes; 10 KB slack for the web build, 50 KB
  for the rest), or over this machine's best. `air_rebuild_cpu` (one line
  changed, debug rebuild) and `air_live_css_s`, `air_live_rsx_s`,
  `air_live_logic_s` (`ax dev`'s own reload and hot-patch, headless) are
  gated against this machine's rows only, 50% over their median: on a
  runner with none they are printed, never failing.
- `scripts/dx-metrics --apk`: the same for the air template's APKs
  (`ax build --android`, each checked, `air_apk_b` the arm64 one).
  `ci.sh` runs it where the NDK and Rust's three Android targets are; in
  GitHub CI the `air-apk` job does (`rustup target add
  armv7-linux-androideabi x86_64-linux-android`, the runner's SDK).

A base before air phase 3 has no shell on air: its air sizes are said
as not gated. What it costs: about 6 minutes more in the `check` job on
a GitHub runner (the air template's debug build, three rebuilds, the
live edits, its release and web builds, and the base's release and web
builds), the `air-apk` job about 20 minutes beside it.

## Not in aio-rs

- aio's `deno task check` / `deno task test` pair and their workflow
  template: aio-rs has `ax lint` and `ax test` (neither `am create` nor
  `ax create` writes a CI file; `ax release <app> --workflow` writes one).
- aio's lab boots throwaway containers and VMs; `ax lab` uses machines
  you already have.
- aio's one-command validation of the whole release matrix: the hand
  checks above stay a list.

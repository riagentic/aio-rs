# Troubleshooting

Symptom → cause → fix, first for every OS, then per OS. For finding out
what a running app did (errors, logs, reports, replay), see
[debugging](README.md).

## First, ask ax

```sh
ax setup --check      # a tool missing on this machine, and the command that gets it
ax errors notes       # ax dev's failed build, then the newest warnings and errors
ax doctor notes       # is it the code on disk; who decided each setting
ax health notes       # healthy, or why not
ax logs notes --level warn --since 15m
```

Each says what it found in words, with `--json` for scripts.

## Which page to start from

| What you see | Start with |
|---|---|
| a red line, a refusal, a panic | [errors](errors.md) |
| `#[methods]` refuses a method's name (`` `start` is also `aio::Handle::start` ``) | rename it ([cells: naming](../state/cells.md#naming)) |
| the window is blank, frozen or slow | [a frozen or blank window](#a-frozen-or-blank-window) |
| the window shows old data | [stale data](#stale-data) |
| a call or a click takes long | [slow calls](#slow-calls) |
| memory keeps going up | [memory](#memory) |
| a client can't reach the server, or drops | [connections](#connections) |
| a view renders too often | [too many renders](#too-many-renders) |
| a cell takes no calls any more | [a cell stopped working](#a-cell-stopped-working) |
| nothing failed, yet something didn't happen | [silent failures](#silent-failures) |
| `WARN [perf]` lines | [budget and storm warnings](#budget-and-storm-warnings) |
| it fails only on one OS | the OS's table below |

## By kind

### A frozen or blank window

| Cause | How you tell | Fix |
|---|---|---|
| a method holds the cell's thread | `WARN [perf]` with the method and its phases; vitals hint `The call notes.import took 840 ms` | move the slow part into an effect ([effects](../state/effects.md)); budget it with `[perf.methods]` if it is meant to be slow |
| the effect queue backed up | vitals hint `The effect queue backed up` | debounce (`cx.after` with one key) or `Mode::Newest` |
| the page's own code blocks it | vitals hint `The client's main thread was blocked`; `ax eval <app> "performance.now()"` is slow to answer | profile the page; less DOM work per render |
| a panic in a view | a debug build's corner badge (`1 page error`); `ax logs <app> --client` | fix the view; refusals are `Result`s, not panics |
| the renderer crashed | `window:crashed` in `ax report <app>` | the report has the page's last lines |
| nothing is drawn at all | `ax shot <app>` is empty; `ax surface <app>` lists nothing | `ax errors <app>` (a failed build), `ax logs <app> --client` |

### Stale data

| Cause | How you tell | Fix |
|---|---|---|
| no `use_live()` in the root component | calls from the view show at once, but changes from effects and schedules show only after the next click | call `use_live()` once in the root ([views](../ui/views.md)) |
| a value read once, not in the render | it never changes after the first render | read it with `part(…)` in the render, not in `use_hook` |
| the copy is offline | `use_status()` says `Offline`, or `Blocked` (so many calls wait that new ones are refused) | it catches up on reconnect; `ax clients <server>` shows who is linked |
| the view shows another copy than you think | `ax state <app> <cell>` differs from the window | name the copy: `<app>@<profile>`, `--instance`, the same `AIORT_DATA` |
| a `visible` filter hides it from this user | `ax state <app> <cell> --as <user>` lacks it | that is the rule working; check the filter ([visibility](../state/visibility.md)) |

### Slow calls

`ax cost <app>` gives each method's p95; `logs/perf.log` says where a slow
call's time went:

| Phase | Means | Fix |
|---|---|---|
| `clone` | copying the state before the call (a call works on a copy, so a refusal leaves no trace) | smaller state; feature `imbl` for big collections (O(1) copies); move rows to a `#[table]` ([sql](../persistence/sql.md)) |
| `apply` | the method itself | less work per call; the rest in an effect |
| `check` | the cell's invariants | cheaper checks, or check only what the call touched |
| `commit` | not budgeted (the disk's) | `#[coalesce]` for rapid small calls ([persistence](../persistence/README.md#coalesced-writes)) |

### Memory

`ax heap <app> --watch` shows the process, each cell's state and events;
`ax health <app>` says `memory: high`, `growing` or `critical`, naming the
biggest cells and gauges. A state that only grows, a cache without
eviction and a long `Vec` of rows are the usual causes;
[production: memory](production.md#memory) has the thresholds.

### Connections

| Symptom | Cause | Fix |
|---|---|---|
| refused with `ACCESS_DENIED` | no key, a wrong one, or an ended session | `ax profile <app>` for a client profile; sign in again |
| refused before any key is looked at | the Host is not in `[server] hosts`, or (with `strict_origin`) the Origin isn't the app's | add the name to `hosts` ([security](../auth/README.md)) |
| a browser warns about the certificate | the app's own root isn't trusted on that machine | `ax trust <app>` explains how to trust it |
| calls fail with `CONNECTION_LOST` | the link dropped mid-call | the call may have run: read the state, then retry |
| the server's log says `transport frozen` | the client did not ping in time; on loopback, its own main thread was busy | see [a frozen window](#a-frozen-or-blank-window) |

### Too many renders

`ax components <app> --renders` lists the last 200 renders and how long
each took, and a debug build says it in the log (`INFO [hint] … a render
reads all of tasks, so it re-renders on every commit`). A component that
renders on every commit reads the whole state
(`tasks()`) where it needs one field: read that field with `part(|t|
&t.field)` instead, and give rows their own component with props
([performance](../performance.md)).

### A cell stopped working

| Its calls are refused with | Cause | Fix |
|---|---|---|
| `<cell> is disabled` | the breaker tripped after repeated failures (a `tripped` report) | fix the cause in the log, then `Cells::enable` ([errors: the breaker](errors.md#the-breaker)) |
| `paused by time travel` | someone stepped it with `ax timetravel` | `ax timetravel <app> <cell> resume` |
| `the app is closing; retry after … ms` | the app is shutting down | retry, or wait for the restart |
| `refused during a call storm` | `[perf.storm] breaker` and a runaway loop | find the loop ([below](#budget-and-storm-warnings)) |

### Silent failures

Nothing went red, yet something didn't happen:

- A refusal the user never saw: `act!(save())` logs a refusal and gives
  `None`, so the log has it but the window shows nothing. Call the
  method through the handle where the user must see why (`e.why`).
- A best-effort part keeps failing: `ax health <app>` names it once it
  failed 5 times in a row (`aio::health::part`).
- An effect's or schedule's refusal is logged, not shown: `ax errors
  <app>`, `ax report <app>`.
- A request to another host is refused: its host isn't in
  `[capabilities] network`.

### Budget and storm warnings

| Line | Means | Fix |
|---|---|---|
| `WARN [perf] notes.import took …` | a method over its `[perf]` budget | move work into an effect, or `[perf.methods] "notes.import" = 2000` |
| `WARN [perf] … effect …` | an effect step held a worker thread too long | `tokio::task::spawn_blocking` for blocking work; await I/O |
| `call storm: notes.sync …` | the method is called `rate` times a second, `secs` in a row | a watcher reacting to its own write: break the loop; `breaker = true` refuses it meanwhile |
| `[budgets] state …` | a cell's saved state passed `[budgets] state` | smaller state, or rows in a `#[table]` |
| `aio.toml [perf.methods] "x.y" names no method of x` | a typo in a budget | fix the key |

## Every OS

| Symptom | Cause | Fix |
|---|---|---|
| A clone fails: no aio-rs at `../…/crates/aio` | the app was made with `--aio` (a checkout beside it) | `ax framework <version>`: aio-rs from git ([setup](../basics/setup.md#where-an-app-gets-aio-rs-from)) |
| `ax create --aio` refuses: the app and the checkout share no folder | a path through the root would name this machine's folders | put the app and the checkout in one folder, or leave out `--aio` |
| the build fails naming an `aio.toml` key | `aio.toml` is parsed as the app builds: a typo or a refused value | fix that key ([reference](../reference/aio-toml.md#common-mistakes)) |
| the build fails at link time (`cc`, `link.exe` not found) | no C toolchain | [setup: Rust and a C toolchain](../basics/setup.md#1-rust-and-a-c-toolchain) |
| the app starts with no data | `[app] id` or a cell's `id` changed | change it back: ids are where the data is |
| a second launch shows nothing new | the app was running: a second launch hands over to the first and ends | that is by design; `--instance <n>` runs a private copy, `--takeover` asks the running one to stop and takes its place |
| a server won't start: the port is in use | another process has `[server] listen`'s port | stop it, or `--port <n>` for this launch |
| `ax dev` rebuilds and restarts on every edit | dx is missing, or the edit can't be patched (a new dependency, a changed type) | `ax setup` fetches dx; a rebuild for the rest is expected |
| a CSS edit doesn't show under `ax dev` | the CSS is kept in `use_hook` (made once) | `aio::use_css!` in the render |
| `ax state` / `ax call` say the app isn't running | it runs from another home (a profile, an instance, `AIORT_DATA`) | name that copy: `notes@<profile>`, `--instance`, the same `AIORT_DATA` |
| `ax shot`, `ax eval` can't reach the window | a release build has no dev door | run it with `ax dev`, or launch with `--cdp` (`ax start notes --cdp`) |
| a UI test fails on a refusal at its end | `TestUi` / `TestCells` fail on refusals nobody looked at | take the expected ones with `failures()` ([testing](../testing/README.md)) |
| an outbound request is refused | its host is not in `[capabilities] network` | add the host there |
| an update is never offered | `[updates] url` unset, the release not signed by one of `keys`, or blocked on data the release can't read | `ax update notes` says the state and the reason |

More pitfalls, by symptom: [FAQ and pitfalls](../basics/faq.md#pitfalls).

## Linux

| Symptom | Cause | Fix |
|---|---|---|
| the window never opens; `ax setup` names a library (`libgtk-3`, `libnss3`, `libgbm`, `libasound`) | Electron's system libraries are missing (servers, minimal images) | install the packages `ax setup` names |
| the log says `starting it with --no-sandbox` | Chromium's sandbox can't start: unprivileged user namespaces are restricted (Ubuntu 23.10+ AppArmor, Debian's switch, a container) and `chrome-sandbox` is not setuid root | expected, and the page is the app's own UI; `AIORT_SANDBOX=1` refuses instead, or make `chrome-sandbox` root-owned with mode 4755 |
| an agent's or CI's window shows on the person's screen | it ran with `--display current` | leave `--display auto` (no terminal: a nested display); install Xvfb or Xephyr (`ax setup` checks) |
| `ax build --deb` fails | no `[app] maintainer`, or no `dpkg-deb` | set `maintainer = "Name <email>"`; install `dpkg` |
| `ax build --appimage` fails | no `mksquashfs` | install `squashfs-tools` |
| `ax build --windows` fails | no MinGW cross linker or rustup target | the error names the package (`mingw-w64`) and the `rustup target add` line |

## Windows

| Symptom | Cause | Fix |
|---|---|---|
| `install.ps1` won't run | PowerShell's execution policy | `powershell -ExecutionPolicy Bypass -File install.ps1` |
| `install.ps1` says there is no `.cargo\bin` | Rust isn't installed yet | `winget install Rustlang.Rustup`, then run it again |
| builds fail with `link.exe` not found | no Visual Studio Build Tools (the C++ workload) | [setup](../basics/setup.md#1-rust-and-a-c-toolchain); `ax setup` checks with `vswhere` |
| SmartScreen warns about the installer or the app | aio-rs 0.1.0's prebuilt archives and app bundles are not OS-signed yet | "More info" → "Run anyway"; build from source to avoid it |
| Windows Firewall asks when a server starts | it listens beyond loopback (`--expose`, `--host 0.0.0.0`) | allow it on private networks; loopback needs no rule |
| an app won't stop from a script | killing by name skips its final save | `ax stop notes` (any OS) |

## macOS

| Symptom | Cause | Fix |
|---|---|---|
| builds fail: no `cc` | no Command Line Tools | `xcode-select --install` |
| "can't be opened" / "unidentified developer" for a downloaded app | the bundle is signed ad hoc, not with a Developer ID (not notarized yet) | Control-click → Open once, or `xattr -dr com.apple.quarantine Notes.app` |
| `ax discover` or pairing finds nothing on the LAN | macOS asks before an app uses the local network | allow it in System Settings → Privacy & Security → Local Network |
| `ax build` can't make a `.dmg` or a macOS bundle on Linux | they are made on a Mac | build on one, or set `[build.macos] host` (or `AIO_MACOS_SSH`) to a Mac over SSH |

## Android

| Symptom | Cause | Fix |
|---|---|---|
| `ax build --android` fails before compiling | no Android SDK, NDK or JDK, or no `aarch64-linux-android` target | the build names what is missing; `ax setup` names the `rustup target add` line |
| `ax install --android` finds no device | no phone attached with USB debugging, or only an emulator | attach one (`adb devices` lists it); `--device <serial>` picks one; `--emulator` allows an emulator |
| `ax install --android` refuses the APK | the newest APK is older than the sources | `--build` builds first, or `--apk <file>` names one |
| the camera never asks | the APK doesn't declare it | `[build.android] camera = true` |
| a `notes://` link doesn't open the app | the scheme isn't declared | `[build.android] schemes = ["notes"]` |
| a new build won't install over the old one | it is signed by another key | keep one keystore (`[build.android] keystore`), and never change `application_id` |

## Web (`ax build --web`)

| Symptom | Cause | Fix |
|---|---|---|
| `ax dev --web` / `--web` builds fail at once | no `wasm32-unknown-unknown` target or no wasm-bindgen 0.2.129 | `ax setup` fetches wasm-bindgen and names the `rustup target add` line |
| the data differs between two tabs' addresses | the cell is kept in the browser's storage, which is per origin (scheme, host and port) | serve it from one address |
| the page shows no error, just nothing | a release web build keeps errors in the console | `ax build --web --dev`: a debug build that shows its errors on the page |

## Still stuck

`ax report notes` lists the app's problem reports (what it filed by
itself when it broke); `ax feedback` says where findings about aio-rs
itself go (`ax feedback notes --create`: the app's own file for them).

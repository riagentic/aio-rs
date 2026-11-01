# Install and set up

What a machine needs to make aio-rs apps (`ax create → ax dev → ax test →
ax build`), how to install `ax`, and how to take it away again.

## 1. Rust and a C toolchain

`ax` builds every app with cargo, and SQLite and ring compile from C, so
both come first:

| | Linux | macOS | Windows |
|---|---|---|---|
| Rust | rustup: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` | rustup (the same line) | `winget install Rustlang.Rustup` (the msvc host) |
| C compiler + linker | `build-essential` (Debian, Ubuntu), `gcc` (Fedora), `base-devel` (Arch) | Command Line Tools: `xcode-select --install` | Visual Studio Build Tools with the C++ workload: `winget install Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools …"` |

On Linux, Electron also needs the libraries a desktop distribution has:
GTK 3, NSS, GBM, ALSA, XSS, XTST. `ax setup` names any it can't load and
the package that has it.

## 2. Install ax

Pick one. Both put `ax` in `$CARGO_HOME/bin` (`~/.cargo/bin`), which
rustup already put on `PATH`.

**Prebuilt** (seconds, no clone): Linux x86_64 (on macOS, 0.1.0 has no
prebuilt `ax` yet: use the source install below, and the installer says
so):

```sh
curl -fsSL https://github.com/riagentic/aio-rs/releases/download/v0.1.0/install.sh | sh
```

Windows x86_64: download `install.ps1` from the
[release](https://github.com/riagentic/aio-rs/releases/tag/v0.1.0), then
`powershell -ExecutionPolicy Bypass -File install.ps1`.

Each archive's SHA-256 is pinned in the installer and checked before
anything is unpacked or run: a wrong checksum installs nothing and says
so. A platform with no prebuilt `ax` or a failed download installs nothing
and prints the source build instead. Run beside a release's files (`sh install.sh`), the installer
takes them instead of downloading. Then it runs `ax setup`.

**From source** (minutes):

```sh
git clone https://github.com/riagentic/aio-rs
cargo install --locked --path aio-rs/crates/aio-cli
```

The package is `aio-rs-cli`, its binary `ax`.

### Upgrading ax

```sh
ax upgrade                    # to aio-rs's newest release
ax upgrade 0.2.0              # to that release (again, if it is this one)
ax upgrade --aio ../aio-rs    # ax built from a checkout (plain ax upgrade returns)
```

`ax upgrade` takes the release's install script, checks its SHA-256
against the release's `SHA256SUMS`, and runs it: the script checks the
archive's own SHA-256 before anything is unpacked. An app keeps its own
aio-rs until `ax framework` moves it (below).

### Where an app gets aio-rs from

aio-rs is not on crates.io: an app `ax create` makes takes it from git,
at this `ax`'s release tag, in both of its `Cargo.toml` files:

```toml
aio = { package = "aio-rs", git = "https://github.com/riagentic/aio-rs", tag = "v0.1.0", features = ["kit"] }
```

`Cargo.lock` pins the commit. A clone builds on any machine, with or
without an aio-rs checkout, and nothing of the machine the app was made on
is in it. To pick or change the version (aio's `am pin`):

```sh
ax create notes --aio-version 0.2.0   # another release (main: the branch; latest: the newest)
ax framework                          # in the app: where each Cargo.toml takes aio-rs from
ax framework 0.2.0                    # every one moved to that release, Cargo.lock with it
ax framework --aio ../aio-rs          # to a checkout, co-developing aio-rs
```

`--aio <dir>` names the checkout relative to the app (`../aio-rs/crates`),
never by its absolute path; the app and the checkout must share a folder
below the root. A clone that holds such a path and has no checkout beside
it: `ax framework <version>` moves it back to a release. A move cargo can't
resolve (no such tag, no network) changes nothing.

## 3. ax setup

```sh
ax setup            # checks every row below, fetches what ax can fetch
ax setup --check    # only looks, fetches nothing
ax setup --json     # the same, for scripts and agents
```

Each missing piece is one line with the command that gets it; the exit
code is 1 until every needed one is there.

### Needed

| | Linux | macOS | Windows | What for | ax setup |
|---|---|---|---|---|---|
| Rust | rustup | rustup | rustup (msvc) | builds every app | checks `rustc -vV` |
| C compiler + linker | gcc or clang | Command Line Tools | Build Tools, C++ workload | links every app | checks `cc` / `xcrun --find cc` / `vswhere` (a gnu host: `gcc`) |
| Electron runtime (pinned, shared by every app) | fetched | fetched | fetched | every window and UI test | fetches it (SHA-256 from Electron's list), then starts it once (`--version`) |
| Electron's system libraries | GTK 3, NSS, GBM, ALSA, XSS, XTST | – | – | the runtime starts | names the library it can't load, and the packages |

### Fetched, optional, or for one target

| | Linux | macOS | Windows | What for | ax setup |
|---|---|---|---|---|---|
| dx 0.7.10 | fetched | fetched | fetched | `ax dev`'s hot-patching (a small crate's edit in under a second; without it: a rebuild on change) | fetches it (SHA-256 pinned in ax) |
| git | package | Command Line Tools | `winget install Git.Git` | build numbers `M.m.<commits>` and releases (without it: `M.m.0-nogit`, no release) | checks |
| ffmpeg | package | `brew install ffmpeg` | `winget install Gyan.FFmpeg` | `ax test --video`, `ax shot --video` | checks |
| Xephyr or Xvfb | package | – | – | an agent's windows off the person's screen | checks |
| wasm-bindgen 0.2.129 | fetched | fetched | fetched (x86_64) | `ax dev --web`, `ax build --web` | fetches it (SHA-256 pinned in ax) |
| aio-rs's Rust toolchain + `wasm32-unknown-unknown`, `aarch64-linux-android`, `x86_64-linux-android` | rustup | rustup | rustup | the web and Android builds | names the `rustup target add --toolchain …` line |
| Android SDK, NDK, JDK | yes | yes | – | `ax build --android`, `ax dev --android` | the build says what is missing |
| `x86_64-w64-mingw32-gcc` + its rustup target | yes | yes | – | `ax build --windows` (cross-built) | `ax build --windows` names the package |
| `dpkg-deb`, `mksquashfs` | yes | – | – | `ax build --deb`, `--appimage` | the build says what is missing |

Every app `ax create` makes pins aio-rs's Rust in its
`rust-toolchain.toml`, so one build folder per machine serves every app.

### What aio-rs doesn't need

- Node, npm or Deno: nothing of aio-rs runs on them.
- A system Chromium for UI tests: they draw in the pinned Electron.
- OpenSSL or pkg-config: TLS is rustls.
- `cargo install dioxus-cli`: `ax setup` fetches the pinned dx.
- curl for AppImages: aio-rs fetches the AppImage runtime itself (SHA-256 checked).

## 4. Typing the commands on each OS

The guide's commands are written for a POSIX shell (Linux, macOS, Git
Bash). `ax …` is typed the same in every shell; only the shell around it
changes. In Windows PowerShell 5.1:

| The guide shows (sh) | Type in PowerShell |
|---|---|
| `a && b` | one command per line (5.1 has no `&&`), or `a; if ($?) { b }` |
| `AIORT_DATA=/tmp/try ax dev notes` | `$env:AIORT_DATA = "$env:TEMP\try"; ax dev notes` (set for that window) |
| `export X=1` / `unset X` | `$env:X = '1'` / `Remove-Item Env:X` |
| `~/.local/share/aio-rt/…` | `$env:LOCALAPPDATA\aio-rt\…` ([where things are](#where-things-are)) |
| `tail -f <log>` | `Get-Content -Wait -Tail 20 <log>`, or `ax logs notes --follow` |
| `grep ERROR <log>` | `Select-String -Path <log> -Pattern ERROR`, or `ax logs notes ERROR` |
| `curl -fsSL …/install.sh \| sh` | `install.ps1`, above |
| `./notes --port 8080` | `.\notes.exe --port 8080` |
| `systemctl`, `chmod`, `sudo` | no twin: see [run at startup](#run-at-startup-or-as-a-service) |

`ax logs`, `ax data`, `ax state` and the other verbs read the same files on
every OS, so a script that uses them needs no per-OS paths.

## Where things are

aio-rs keeps everything in the OS's own folders, under `aio-rt`. That is
never aio's `~/.<appId>`, so an aio app and its aio-rs build run side by
side (a ported app, `[app] port_of`, runs on its aio home,
[data](../migration/data.md); the Electron runtime is in aio's store, shared
with aio's apps, [desktop](../clients/desktop.md#one-store-with-aio)). `~` is
`$HOME`, and on Windows `%USERPROFILE%` when `HOME` is unset.

| What | Linux | macOS | Windows |
|---|---|---|---|
| aio-rs's tool home | `~/.local/share/aio-rt/` (`$XDG_DATA_HOME`) | `~/Library/Application Support/aio-rt/` | `%LOCALAPPDATA%\aio-rt\` |
| an app's home: `data/`, `logs/` | `<tool home>/apps/<id>/` | the same | the same |
| an app's cache (rebuildable) | `~/.cache/aio-rt/<id>/` (`$XDG_CACHE_HOME`) | `~/Library/Caches/aio-rt/<id>/` | `%LOCALAPPDATA%\aio-rt\cache\<id>\` |
| the window's browser profile | `<app home>/data/electron/profile/` | the same | the same |
| a private instance (`--instance n`) | `<tool home>/instances/<n>/<id>/` | the same | the same |
| the shared Electron runtime (aio's store, shared with aio's apps) | `~/.local/share/aio/electron/<version>-<os>-<arch>/` (`$XDG_DATA_HOME`) | `~/Library/Application Support/aio/electron/…` | `%LOCALAPPDATA%\aio\electron\…` |
| dx, wasm-bindgen (fetched) | `<tool home>/tools/` | the same | the same |
| ax's one build folder | `<tool home>/target/` (`CARGO_TARGET_DIR` wins) | the same | the same |
| this machine's TLS root (`ax trust`) | `<tool home>/tls/` | the same | the same |
| release signing keys (`ax keygen`) | `~/.config/aio-rt/keys/` (`$XDG_CONFIG_HOME`) | the same | the same |
| an app installed from its bundle | `<tool home>/install/<id>/`, `~/.local/bin/<id>`, a menu entry | `/Applications/<Name>.app` (the app offers the move) | `%LOCALAPPDATA%\Programs\<id>\`, a Start menu entry, Settings › Apps |
| the app's sockets (`ax` talks to it here) | `$XDG_RUNTIME_DIR/aio-rt/<id>-<hash>/`, else the temp dir | the temp dir | named pipes `\\.\pipe\aio-rt-<id>-<hash>-…` |

- `ax data <app>` prints an app's folders and what to back up; `ax doctor
  <app>` says who chose them (a flag, a variable, the default).
- The socket folder's name carries a hash of the data folder, so two copies
  of one app with different data (a `--profile`, an `--instance`,
  `AIORT_DATA`) never ask for the same socket or pipe, on any OS. Both are
  private to the user (mode 0700; the pipe admits only this user).
- `data/` and `logs/` are private (0700). The full layout of an app's
  home, and backups: [files](../persistence/files.md).

**For someone who only uses the app** (no `ax`, no terminal): its data is
`data/` and its logs are `logs/` in the app's home above. Finder hides
`~/Library` (Go → Go to Folder…, then the path); Explorer shows
`%LOCALAPPDATA%` when it is typed in the address bar. aio-rs adds no
"open logs" menu item; an app that wants one builds it.

## Stopping an app

A clean stop runs the cells' `#[on_stop]` methods and writes what a
coalesced method has not written yet (at most `aio::PERSIST_DEBOUNCE`,
100 ms, of calls). A hard kill skips both. A plain (not coalesced) method
is on disk before it answers, so a hard kill never loses it.

| How | Linux, macOS | Windows |
|---|---|---|
| `ax stop <app>` | clean: asked over its private socket | clean: asked over its pipe |
| Ctrl-C in its terminal | clean (SIGINT) | clean (Ctrl-C, Ctrl-Break, closing the console) |
| closing the window; `<app> --quit` | clean | clean |
| `ax stop` when the app doesn't go within `--wait` (15 s) | SIGTERM (clean), then SIGKILL after 5 s (**hard**) | the process is ended (**hard**): Windows has no SIGTERM |
| `ax kill <app>` | SIGKILL: **hard** | the process is ended: **hard** |
| `kill <pid>` (SIGTERM) | clean | no twin |
| `kill -9`, Force Quit, Task Manager, `taskkill /F` | **hard** | **hard** |

`ax stop` says which happened: `stopped`, `stopped by SIGTERM: asking
didn't do it within 15 s`, or `killed: neither asking nor SIGTERM stopped
it`. The pid it signals comes from the app's own `health.json`, never from
matching process names. `ax stop --all` stops every app of the workspace;
`ax kill` is for an app that is stuck.

## First launch of a downloaded desktop app

aio-rs 0.1 signs no bundle with a certificate the OS knows, so a
**downloaded** copy is held on its first open (a copy built on the same
machine is not):

| OS | What appears | What to do |
|---|---|---|
| macOS | "cannot be opened because Apple cannot check it" (Gatekeeper) | Control-click the app → Open → Open; or System Settings → Privacy & Security → Open Anyway |
| Windows | "Windows protected your PC" (SmartScreen) | More info → Run anyway |
| Linux | nothing | an AppImage needs `chmod +x` once |

A macOS app opened from its disk image or from Downloads runs from a
read-only place an update can't replace, so once its window is up it
offers, once, to move itself into Applications ("Not Now" is remembered;
`AIORT_MOVE_TO_APPLICATIONS=never` or `move` decides for an unattended
install). The update signature is separate from all of this and always
checked ([release](../build/release.md#signing-for-the-os)).

## The firewall, when a server listens on the network

A server listens on loopback (`[server] listen = "127.0.0.1:7420"`) until
`--expose`, `--host 0.0.0.0` or a `listen` beyond loopback says
otherwise. Then the OS may ask the person at the screen, the first time
that program listens:

| OS | What appears | If it is dismissed or denied |
|---|---|---|
| Windows | a Windows Security dialog asking whether the program may be reached on your networks | a **block** rule is kept for that program: allow it under *Allow an app through Windows Firewall*, or add a rule (below) |
| macOS | "Do you want the application to accept incoming network connections?", only when the firewall is on | blocked: System Settings → Network → Firewall → Options |
| Linux | nothing | a host firewall (`ufw`, `firewalld`) needs the port opened by hand |

A blocked server looks dead from other machines and healthy on its own:
its log, `ax status` and `http://127.0.0.1:<port>` all work. Check from the
second machine first. The program is the app's own binary, so the question
comes again for each newly installed path. With nobody at the screen (a
service) nobody is asked, and Windows blocks.

```powershell
# an administrator PowerShell: allow one installed server on private networks
New-NetFirewallRule -DisplayName 'notes' -Direction Inbound -Action Allow -Profile Private -Program "$env:LOCALAPPDATA\Programs\notes\notes.exe"
```

aio-rs opens no firewall rule and, unlike aio, prints no `firewall:` line
at boot naming what the OS may ask: a gap.

**macOS: Local Network.** macOS 15 and later asks, per app, before it may
reach other devices on the LAN: a window or a terminal that connects to an
aio-rs server on the LAN, `ax discover`, pairing. Denied, the connection
fails as if the server were down while the internet still works. Allow it
under System Settings → Privacy & Security → Local Network. aio-rs names
this permission nowhere when a connection fails: a gap.

## Run at startup, or as a service

`ax build` of a server (`[app] mode = "server"`, built as a `cli`
target) writes its supervisor's file beside the bundle, each with the
line that installs it (`ax build` prints it):

| OS | File | Installs with |
|---|---|---|
| Linux | `<bundle>.service`, a systemd user unit | `cp` it to `~/.config/systemd/user/<id>.service`, `systemctl --user daemon-reload`, `systemctl --user enable --now <id>` (`loginctl enable-linger`: it runs without a login too) |
| macOS | `<bundle>.plist`, a launchd agent | `cp` it to `~/Library/LaunchAgents/app.aio.<id>.plist`, then `launchctl bootstrap gui/<uid> <that file>` |
| Windows | `<bundle>.ps1`, a Task Scheduler script | `powershell -ExecutionPolicy Bypass -File .\<bundle>.ps1 -Install`, as an administrator (`-Uninstall` removes the task) |

Each runs the installed binary (`~/.local/bin/<id>`,
`%LOCALAPPDATA%\Programs\<id>\<id>.exe`) as the user who installed it,
and sets `AIO_SUPERVISED=1` and `AIO_STOP_STAYS_DOWN=1`. What the app does
under them:

| The app | Exits with | The supervisor |
|---|---|---|
| installed an update, or asked to restart | `0` | starts it again (the new version) |
| `ax stop`, SIGTERM, Ctrl-C | `143` (`aio::lifecycle::STAYS_DOWN`) | leaves it down |
| the same, with `AIO_SUPERVISED=1` alone | `0` | starts it again |
| crashed | other | starts it again after 5 s |

Without `AIO_SUPERVISED=1`, an app that installs an update starts the new
version itself; under a supervisor it only exits, so two processes never
compete for one app's lock. A hand-written unit for an app built
another way gets the same contract by setting the two variables and
returning `aio::lifecycle::exit_code()` from `main`, as the service
template does. A window app has no supervisor file: it starts at login
the way the OS starts any app (Login Items, the Startup folder, an
autostart `.desktop` entry); aio-rs adds no "start at login" switch.

## Uninstalling

### ax itself

```sh
ax uninstall            # ax out of $CARGO_HOME/bin
ax uninstall --tools    # also tools/ and the build folder target/ in the tool home
```

Your apps, their data and installs stay (the rest of the tool home,
[where things are](#where-things-are)). The shared Electron runtime is aio's
store, shared with aio's apps: `ax prune` says what of it may go.

### An app

| OS | The program | Its data (deleting it can't be undone) |
|---|---|---|
| Linux | `uninstall.sh` beside its `install.sh`, or `ax remove <id>`: the binary, its versions, the menu entry and icon | `<tool home>/apps/<id>/` and its cache: `uninstall.sh --data`, `ax remove <id> --data` |
| Windows | Settings › Apps → the app → Uninstall (it runs `<id>.exe --uninstall`): the program folder, the Start menu entry | `%LOCALAPPDATA%\aio-rt\apps\<id>\` |
| macOS | drag `/Applications/<Name>.app` to the Bin | `~/Library/Application Support/aio-rt/apps/<id>/` |

- Close the app first: `ax remove` refuses a running app, and Windows
  refuses to delete a running program's folder.
- `ax installed` lists the apps installed from a bundle; `ax remove` takes
  only those (a `.deb` is removed with `apt remove`).
- The shared runtime is not the app's: `ax prune --yes` removes the
  runtimes no installed app names and nothing started from in 30 days.
- `ax backup <app> <file>` first, if the data may be wanted again.

## File names, paths, line endings

- **Case.** macOS and Windows file systems ignore case by default, Linux
  doesn't: `mod store;` beside a file named `Store.rs`, or
  `include_str!("Logo.svg")` for `logo.svg`, builds on a Mac or PC and
  fails on a Linux CI runner. aio-rs doesn't check this: run `ax test` on
  Linux before you ship (the workflow `ax release --workflow` writes
  builds on Linux, so a case mistake fails there, [release](../build/release.md)).
- **Path length.** aio-rs does nothing about Windows' 260-character path
  limit. The shared build folder under `%LOCALAPPDATA%\aio-rt\target`
  keeps build paths short; for a deep checkout, `git config --global
  core.longpaths true`.
- **Line endings.** `ax create` writes no `.gitattributes` (aio's `am
  create` writes `* text=auto eol=lf`): a Windows checkout with Git's
  `core.autocrlf` gets CRLF files. Rust and `aio.toml` read either; add
  `* text=auto eol=lf` yourself to keep the bytes the same on every OS.

## Checked on

- Linux x86_64 (Ubuntu 24.04): the prebuilt install, then `ax setup`
  into an empty home: 62 s, runtime and dx included.
- Windows 11 (no Rust, no compilers): `install.ps1` installed the
  prebuilt `ax` and refused a wrong pin; `ax setup` fetched the runtime
  and dx (56 s) and named Rust and the Build Tools as missing; the
  uninstall above left nothing. The workspace tests, UI tests included,
  pass there.
- macOS 14 x86_64: `ax setup --check` and the workspace tests, UI tests
  included, pass. Not checked yet: Apple silicon.

Problems on one OS: [troubleshooting](../debugging/troubleshooting.md).
Next: [quickstart](quickstart.md).

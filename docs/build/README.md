# Develop, build and install

From a saved edit to an installable app: `ax dev` while you work,
`ax build` for what you ship, `ax install` to try it as a user would.
Releasing and updating: [release](release.md). What each OS, the web and
Android need: [targets](targets.md).

## Develop: `ax dev`

```sh
ax dev notes                     # build, run; edits reach it live
ax dev notes --instance try --fresh   # a private copy, on new, empty data
ax dev notes --web               # the web build on loopback, its tabs live
```

`ax dev <app>` builds and runs the app. A saved edit reaches the running
app, its state kept: `rsx!` markup at once (~3 ms, nothing compiled),
Rust in every workspace crate (views and cell methods alike) hot-patched
by ax itself (Linux, macOS). An edit a patch can't take (a
`Cargo.toml`), or Windows: the app is built again, and a good build
restarts it (its state comes back from the log) while a failed one
leaves the running app alone. A saved stylesheet shows at once, no
compile ([theming](../ui/theming.md)); with `[build] css`, its toolchain
runs first. After each patch or build `ax dev` says how long it took
from the save (`ax dev: in 0.6 s from the save`).

### What an edit costs

A Rust edit recompiles the whole crate it is in (rustc reruns every
macro, `rsx!` included, and rewrites the crate's metadata), so its time
grows with that crate, not with the edit. Measured with `ax dev` in
0.1 (headless, 32 cores; the two bigger apps build their libraries at
`opt-level = 1`, so their first build is longer); the compile, which is
most of each, is the same in 0.2, and the template's patch now lands in
~0.3 s ([dev](dev.md)):

| The edited crate | A patch lands | A start after an edit | A start, nothing changed | First build |
|---|---|---|---|---|
| `ax create`'s template (80 lines) | 0.6 s | 1.5 s | 1.4 s | 15 s |
| 4 000 lines, 68 `rsx!` | 1.4 s | 2.3 s | 1.5 s | 33 s |
| 74 000 lines, 576 `rsx!` | 9.4 s | 10 s | 1.7 s | 60 s |

Keep a big app fast to edit by splitting it into crates of a few
thousand lines (its views by area, say), each a path dependency and a
member of the app's workspace. `ax add <app> page <name>` makes one
([ax add](../clients/ax.md#ax-add)), and `ax lint` hints at a crate of
more than 15 000 lines. An edit then recompiles its own crate alone and
is patched in; the crates built on it compile after, off the time to the
screen ([dev](dev.md#hot-patching)). So keep the often-edited code
(views, pages) in leaf crates, and the shared core small and stable.
`ax dev` watches every workspace crate the app uses, and patches the ones
that changed.

Measured in 0.4.0 with `ax dev` (headless, CARGO_BUILD_JOBS=4 on 12
shared cores, from the save to the window showing it, medians of 4-5):

| The app (~74 000 lines, 660 generated modules, each a 60-line `rsx!` and plain Rust) | An edit lands |
|---|---|
| one crate | 5.5 s (rustc 4.8-5.4 s of it: its whole front end) |
| 10 page crates and a kit crate under them: a page's edit | **0.48 s** (rustc ~0.4 s) |
| … an edit in the kit every page calls | 1.0-1.9 s (most of it the patch of every page's caller of it: 94 000 functions) |

Before 0.4.0 an edit compiled every crate built on the edited one before
its patch: a page's edit 1.9 s, one in the shared kit or core 8-14 s
(measured in 0.1).

Splitting has a cost in release builds: generic code, async functions
included, is copied into each crate that uses it, so the binary grows a
little (that app: 15.8 → 16.4 MB).

| Flag | Does |
|---|---|
| `--instance <n>` | a private copy with its own data |
| `--fresh` | with `--instance`: that copy on new, empty data (never the app's own) |
| `--size 390x844` | the window at that size for this run (its saved size untouched) |
| `--web` | the web build (debug) on loopback, its tabs live |
| `--android [--device serial] [--native]` | on an emulator or phone: the web build in a dev APK installed once, each edit reloading it (`--native`: the app itself, rebuilt for that CPU on every save) |
| `--display auto\|isolated\|current\|:N` | where the window goes; with no terminal (an agent) a nested display, never the person's |
| `--cdp[=port]` | the window's DevTools protocol on loopback, for other tools |
| `--watch=false`, `--watch=src/ui,assets` | build once, or rebuild on changes there only |

Args after the app go to the app (`ax dev notes -- --route /users/7`).
Every flag, the app's own launch flags, and which code goes into which
build: [development mode](dev.md).

- `ax start <app>`, `ax stop`, `ax restart`, `ax status`: the app in the
  background (its output in `logs/start.log`); `--all`: every app of the
  workspace.
- `ax errors <app>`: the failed build, then the newest warnings and
  errors. More in [debugging](../debugging/README.md).

## Build: `ax build`

```sh
ax build notes                   # an installable app for this OS, in dist/
ax build notes --list            # the target kinds, and the app's targets
ax build notes --smoke           # then start each Linux bundle headless and stop it
ax build notes --sizes           # the release binary against its budget
ax build notes --quick           # a release to try in seconds: no LTO, bigger; never to ship
```

Without `[build] targets` it makes this OS's bundle,
`dist/<id>-<version>-<os>-<arch>.*`. The kinds:

| Kind | Makes |
|---|---|
| `desktop` | this OS's bundle: Linux `.tar.gz` (with `install.sh`, `uninstall.sh`), macOS zipped `.app`, Windows one-click `.exe` and a `.zip` |
| `windows` | the Windows `.exe` and `.zip`, cross-built from Linux (`--windows`) |
| `deb` | a Debian package (`--deb`; needs `[app] maintainer`) |
| `appimage` | one portable Linux file (`--appimage`) |
| `dmg` | a macOS disk image (on a Mac) |
| `cli` | a headless binary on `PATH`, no menu entry (a CLI or a server; a server also gets its supervisor file) |
| `android` | signed APKs, one per CPU (`--android`) |
| `web` | the installable web app (PWA) in `dist/<id>-web/` (`--web` alone) |

`--web` makes the web app alone; as a target (`targets = ["desktop",
"web"]`) one `ax build` makes it beside the others (aio's `build-all`).
A Linux bundle says the AppImage beside it: `--appimage` is aio's default
artifact, one portable file.

```toml
[app]
id = "notes"
name = "Notes"
version = "1.4"
maintainer = "Notes Team <team@example.org>"

[build]
targets = ["desktop", "deb", { name = "admin", kind = "cli", bin = "notes-admin" }]
platforms = ["linux", "windows", "macos-arm64"]
budget_mb = 12
smoke = true
runtime = "shared"         # or "bundled": Electron inside the package, for offline machines
chromium_extras = "keep"   # or "strip" (a bundled runtime without the GPU extras)

[build.splash]
image = "assets/splash@2x.png"
text = "{title}"

[build.assets]
"/media" = "media"         # media/song.mp3 is the app's /media/song.mp3
```

- `targets`: kinds, or `{ name, kind, bin }` to build two of a kind or
  another binary of the workspace (one repo, several apps).
  `--targets a,b` builds some of them.
- `platforms`: each desktop target for each of `linux`, `linux-arm64`,
  `windows`, `windows-arm64`, `macos`, `macos-arm64`, `host`
  (`--platforms` for one run). A macOS `.zip` is made anywhere (zig
  links it away from a Mac); a `.dmg` on a Mac: `[build.macos] host` (or
  `AIO_MACOS_SSH`) names one over ssh.
- `budget_mb`: the desktop binary's size budget (default 10, 15 on Windows); a build
  above it fails and names the crates that take the space. Size with
  `--sizes`, not a plain `cargo build --release` (it lacks ax's link
  flags and is bigger).
- `smoke` (`--smoke`): each Linux bundle, `.deb` and AppImage is started
  headless, stopped and its log checked; `ax smoke <artifact>` does one
  file and reports its boot time and memory.
- `runtime`: a desktop app's Electron is `shared` (one per machine,
  fetched at its first start) or `bundled`.
- `[build.splash]`: the app's image (and text) from its start until its
  window has loaded, on the desktop, the web and Android; `min_ms`,
  `max_ms`, `background = ["#light", "#dark"]`. A web build also makes
  iOS's launch images from it (`apple-splash-<w>x<h>.png`, one per
  iPhone and iPad screen), so a Home Screen app starts on the same picture.
- `assets` (aio's `assets`): folders beside `aio.toml` built into the app
  by URL prefix. The window (`aio://app/media/…`), the app's server
  (public, as its page is), the web build (cached offline) and the APK
  answer each file at the same path; dotfiles are left out, a name with
  `:` or `*` is refused. A file added later is built in at the next `ax
  build`, `ax dev` or `ax start` (they tell cargo; a plain `cargo build`
  doesn't see a new file). `[server] dirs` serves folders at run time
  instead, behind the key.

### Release build time

The template's `[profile.release]` (fat LTO, `opt-level = "z"`, one
codegen unit, stripped) makes the smallest binary, so `ax build` keeps
it. Its cost is the link: the whole program is optimised as one unit,
on one core, at each build. Measured on the template (4 build jobs, a
loaded machine, `cargo rustc --release` with ax's link flags):

| `lto`, `codegen-units` | Clean build | One line changed | Binary |
|---|---|---|---|
| `true` (fat), 1: the template's | 56 s | 27 s | **5.42 MB** |
| `"thin"`, 1 | 46 s | 10 s | 6.23 MB (+15%) |
| `"thin"`, 16 | 40 s | 12 s | 6.83 MB (+26%) |
| `false` (within each crate), 16 | **35 s** | **2.1 s** | 7.43 MB (+37%) |
| `true` (fat), 16 | 60 s | 31 s | 5.70 MB |

`opt-level = "s"` was bigger and slower to build than `"z"`. To try a
release build quickly, not to ship it, `ax build --quick` builds every
target (desktop, Android, web) without LTO, in 16 codegen units: the
template's `ax build` after a one-line change takes 2.2 s, from 27 s.
Cargo keeps both builds apart, so going back and forth rebuilds nothing.
`--sizes` and a Mac over ssh refuse it; `ax release` never builds so.

```sh
ax build notes --quick --smoke
```

### What lands in `dist/`

| File | From |
|---|---|
| `<id>-<version>-linux-<arch>.tar.gz` | `desktop` on Linux: the binary, its icon, `install.sh`, `uninstall.sh` |
| `<id>-<version>-macos-<arch>.zip`, `.dmg` | `desktop` / `dmg` on a Mac: the `.app` |
| `<id>-<version>-windows-<arch>.exe`, `.zip` | `desktop` on Windows or `windows`: the one-click download, and the exe alone for updates |
| `<pkg>_<version>_<arch>.deb`, `<id>-<version>-<arch>.AppImage` | `deb`, `appimage` |
| `<id>-<version>-<os>-<arch>-cli.*` and a `.service` / `.plist` / `.ps1` | `cli` (a server's supervisor file beside it; on Linux also `.system.service`, a system unit that starts at boot) |
| `<id>-<version>-android-<abi>.apk`, `<id>-<version>-android.aab` | `android` |
| `<id>-web/` | `--web`, `web` |
| `<id>.manifest.json` | every build: what was built, from which commit, each target's files |

`<arch>` is `x64` or `arm64` (the AppImage and the `.deb` use their own
words: `x86_64`, `amd64`). Each bundle and every other file it needs, per OS: [targets](targets.md).
`dist/` is in the template's `.gitignore`; `ax release` lays out its own
tree under `dist/<channel>/` ([release](release.md)).

### What `ax build` checks

- `aio.toml` parses (it is built into the binary: a typo fails the build,
  naming the key).
- `[build] css` runs first, when set; a failure stops the build.
- The release binary says the version the build gave it: a launcher that
  passes `env!("CARGO_PKG_VERSION")` instead of `aio::app_version!()` to
  `aio_shell::start` is refused (its updates would never match).
- It is within its budget (`budget_mb`; 10 MB, a server 7 MB, each half again on Windows), else it
  isn't packed and the crates that take the space are listed.
- It holds no path of the build machine's home (a shipped binary never
  names who built it).
- Two targets don't write the same file; a platform's linker and Rust
  target are there, else the command that adds them is said.
- A build from uncommitted edits says so: its version ends `-dirty.<hash>`
  and it is never a release.

## Versions

`[app] version = "1.4"` numbers builds from git: `1.4.<commit count>`, so
every commit bumps it and two builds of one commit agree.
`"1.4.2"` is pinned, used as written. Either may end in a stage:
`1.4-beta` builds `1.4.<count>-beta`, ranked below `1.4.<count>`
(`-alpha` < `-beta` < `-rc` < none). Uncommitted edits make
`-dirty.<hash>`, a build outside git `-nogit.<hash>`: prereleases, never
released. Without it, the package's version (`Cargo.toml`) is used.
`aio::app_version!()` is the string the build gave the binary.

## Install and try it

`ax install` builds from source and installs this OS's bundle for this
user: on Linux it runs the bundle's `install.sh` (nothing left aside); on
macOS and Windows it builds the bundle and says how to install it (drag
the `.app` to Applications; double-click the `.exe`).

| Command | Does |
|---|---|
| `ax install <app>` | builds this OS's bundle and installs it for this user (Linux: its `install.sh`, into `~/.local/bin` and the app menu, no root) |
| `ax install <app> --android [--build]` | the newest APK for the attached phone's CPU onto it, then starts it |
| `ax installed` | the apps installed from a bundle on this machine |
| `ax update <app>` | an installed app's versions and update state |
| `ax remove <app> [--data]` | takes an installed app away (not while it runs); its data stays unless `--data` |
| `ax data <app>` | where the app keeps everything, and what to back up |
| `ax smoke <artifact>` | starts one bundle headless as a user would, then stops it: boot time, memory, a clean stop |
| `ax lab <app> windows\|macos\|linux` | the app on a real machine's desktop over ssh, for a person to try (`AIO_LAB_WINDOWS`…) |

## CI for an app

```sh
ax lint                 # what types can't check, plus clippy (exit 1 on errors)
ax test notes           # cargo test, the UI tests included (headless)
ax build notes --smoke  # the bundles start and stop cleanly
```

UI tests run headless in the pinned runtime: no display, no window
manager. `ax release <app> --workflow` writes a GitHub workflow that
builds, signs and publishes on a `v*` tag ([release](release.md#github-workflow)).

## Common mistakes

- Sizing with `cargo build --release`: use `ax build --sizes`.
- `--deb` without `[app] maintainer`: refused, it names the key.
- A `.dmg` from Linux: it needs a Mac (`[build.macos] host`).
- Expecting `ax dev` to keep in-memory view state across a restart: a
  hot patch keeps it, a rebuild starts the views again (the cells'
  state is saved).
- `ax dev --fresh` without `--instance`: refused; the app's own data is
  never thrown away.
- Giving `aio_shell::start` the package's version: `ax build` refuses
  it; use `aio::app_version!()`.

How aio-rs itself is released: [contributing](../../CONTRIBUTING.md#releasing-aio-rs).

# Environment variables

Every variable an aio-rs app, `aio-rs-shell` or `ax` reads, in one table per
use. aio-rs's own start `AIORT_`; the `AIO_` ones are shared with aio (the
Deno-based framework) or name a build tool's secret. Anything not listed here
is not read, except what aio-rs hands its own child processes and tests.

An app's configuration is its `aio.toml`, built in
([reference](aio-toml.md)). A variable only changes what may differ per
launch or per machine: where the data is, the key, the update channel, how
the window draws. Each launch's boot report says every setting and who
decided it (`[aio] boot data … (AIORT_DATA)`); `ax doctor <app>` prints the
ones that move an app's data, key or profile when set (`AIORT_DATA`,
`AIORT_CONFIG`, `AIORT_PROFILE`, `AIORT_TOKEN`, `AIORT_SANDBOX`,
`AIO_APPS_DIR`, `AIO_PROFILE`, `AIO_DISCOVERY_PORT`, `AIO_FEEDBACK_DIR`;
the key as `(set)`, never its value).

Examples set a variable POSIX style, `AIORT_DATA=/tmp/notes-try ./notes`. In
PowerShell: `$env:AIORT_DATA = 'C:\Temp\notes-try'; .\notes.exe`.

## Running an app

Read by the app's process (`aio`, `aio-rs-shell`) at start.

| Variable | Values (default) | Effect |
|---|---|---|
| `AIORT_DATA` | a folder (unset) | All of the app's files under it: `data/`, `logs/`, `cache/`. Tests and portable installs use it; it wins over a ported app's own home rules too, so a sandbox never opens real aio data. Inside an `aio::testing` sandbox only a folder under the temp dir counts. [Where files live](../persistence/files.md) |
| `AIORT_PROFILE` | a name or a path (unset) | A second home beside the default one, `<home>-<name>`: the variable form of `--profile`; the flag wins |
| `AIORT_TOKEN` | a long random string (unset) | The app's key (sync, the local door, `ax`), instead of `data/sync.token`. Under 16 characters or 80 bits it is used and warned about. Never passed on to a child that is not an aio-rs app. [Security](../auth/README.md) |
| `AIORT_UPDATE_CHANNEL` | a channel name (unset) | Follow this update channel. Order: the variable, then the person's own choice (`set_channel`), then the channel the build was stamped with, then `[updates] channel`. [Release](../build/release.md) |
| `AIORT_CONFIG` | anything | Refused: the app stops with an error naming the per-launch flags and variables to use instead. An app reads only the `aio.toml` it was built with |
| `AIO_PROFILE` | a name or a path | aio's profile variable, read only by a ported app (`[app] port_of`), after `--profile` and `AIORT_PROFILE` |
| `AIO_APPS_DIR` | a folder | aio's root of every app's data, read only by a ported app: its home is `<AIO_APPS_DIR>/<aio app>`, as in aio |
| `AIO_DISCOVERY_PORT` | a UDP port (`8099`) | The port LAN discovery broadcasts and answers on; server and client must match. A value that is not a port above 0 is ignored. Needs `tls` |
| `AIO_SUPERVISED` | `1` (unset) | A server or CLI app runs under a supervisor (the systemd unit, launchd agent or Windows task `ax build` writes, which set it): an update restarts by exiting, not by starting its own successor |
| `AIO_STOP_STAYS_DOWN` | `1` (unset) | With `AIO_SUPERVISED=1`: a stop (`ax stop`, SIGTERM) exits 143, so the supervisor leaves the app down; a restart for an update still exits 0 |
| `AIORT_MOVE_TO_APPLICATIONS` | `never`, `off`, `false`, `0`, `no`, `move` (unset: ask) | macOS: whether a desktop app opened from its disk image or Downloads offers to move itself into Applications; `move` moves without asking (an unattended install) |
| `NO_COLOR` | anything | No colour in the log on a terminal ([no-color.org](https://no-color.org)); an empty value counts as unset |
| `FORCE_COLOR` | `0`, `false`, empty: off; else on | Wins over `NO_COLOR` and the terminal test both ways (a pipe that renders ANSI) |

## The window (desktop)

Read by `aio_shell::run` and the shared Electron runtime.

| Variable | Values (default) | Effect |
|---|---|---|
| `AIORT_RUNTIME` | a folder (unset) | Use the Electron runtime in that folder, not the one bundled beside the binary or the shared store's; nothing is downloaded or pruned |
| `AIORT_ELECTRON_MIRROR` | a URL (GitHub's releases) | Where the runtime is downloaded from on first start; what comes is still checked against the SHA-256 the build pinned |
| `AIO_SHARED_ELECTRON_DIR` | a folder (the OS's place) | Moves the shared Electron store, aio's and aio-rs's both (aio reads the same variable) |
| `AIORT_GPU` | `hardware`, `software` (`hardware`; `software` after the GPU failed here) | How the window draws. A GPU that failed once draws in software from the next start until `AIORT_GPU=hardware` asks again |
| `AIORT_SANDBOX` | `1` (unset) | Linux: keep Chromium's sandbox strict, no window rather than one without it. Unset, aio-rs turns the sandbox helper off only where the host can't run it, and says so in the boot report |
| `AIORT_ELECTRON_ARGS` | Chromium switches, space separated | Extra switches from an allow-list: display, GPU, locale and logging ones (`--disable-gpu`, `--disable-dev-shm-usage`, `--ozone-platform=headless`, `--use-angle=…`, `--force-device-scale-factor=…`, `--lang=…`, `--enable-logging`, …). Any other switch is refused and named in the log with why (`--remote-debugging-port`, `--no-sandbox`, `--disable-web-security`, `--proxy-server`, `--js-flags`, …) |
| `AIORT_CDP` | a port (unset) | A debug build's window opens the DevTools protocol on 127.0.0.1 at that port. A release build refuses it. `ax shot`, `ax eval`, `ax surface` and `ax trigger` attach to it when `--cdp` is not given |
| `AIORT_DIALOG` | `none`, `browser` (unset) | The first-run and error windows (the runtime download, Retry or Quit): `none` shows none (the log has it all); `browser` uses a browser page instead of zenity or yad on Linux |
| `AIORT_NO_OPEN` | non-empty (unset) | The app hands nothing to the person's browser or file manager: each open is refused, naming what it would have opened. `ax dev` sets it for a caller with no terminal; a test sandbox has it always |

## `ax` and building

Read by `ax` (the `aio-rs-cli` binary) and the install scripts.

| Variable | Read by | Effect |
|---|---|---|
| `AIORT_DISPLAY` | `ax dev`, `ax start` | Where the window goes when `--display` is not given: `auto` (default: no terminal, a nested display), `isolated`, `current` or `:N` |
| `AIORT_UI_ROOTS` | `ax ui` | More folders to find projects in (stopped apps), `:`-separated (`;` on Windows), beside `~/aio-apps`, the home folder, the current folder and the running apps' folders. [`ax ui`](../clients/ax.md#ax-ui-the-visual-app-manager) |
| `AIORT_SMOKE_DISPLAY` | `ax build --smoke` | A display (`:N`) to start each bundle on, instead of one started for it (Xvfb) or the runtime's headless one |
| `AIO_MACOS_SSH` | `ax build` (macOS targets) | `[ssh options] [user@]host` of a Mac, so another OS builds the `.app` and `.dmg` there |
| `AIO_MACOS_DIR` | `ax build` (macOS targets) | The folder on that Mac, under its home (`.cache/aio-build`) |
| `AIO_WINDOWS_SSH` | `ax build --smoke` | A Windows machine (`[ssh options] [user@]host`) to smoke-test a Windows bundle on |
| `AIO_LAB_WINDOWS`, `AIO_LAB_MACOS`, `AIO_LAB_LINUX` | `ax lab` | `[ssh options] [user@]host` of each lab machine. [Labs](../testing/labs.md) |
| `AIO_ANDROID_KEYSTORE` | `ax build --android` | The release keystore (wins over `[build.android] keystore`); with it the APK is signed with that key, else with this machine's own |
| `AIO_ANDROID_KEYSTORE_PASSWORD` | `ax build --android` | The keystore's password; needed with a keystore |
| `AIO_ANDROID_KEY_ALIAS` | `ax build --android` | The key's alias (wins over `[build.android] key_alias`); needed with a keystore |
| `AIO_ANDROID_KEY_PASSWORD` | `ax build --android` | The key's own password, when it has one |
| `AIO_WINDOWS_CERT`, `AIO_WINDOWS_CERT_PASSWORD` | `ax release --aio` | A `.pfx` and its password: aio-rs's own `ax.exe` is Authenticode-signed (osslsigncode). Unset: unsigned, and said. An app's bundles are not OS-signed yet |
| `AIO_WINDOWS_TIMESTAMP_URL` | `ax release --aio` | The timestamp server for that signature (`http://timestamp.digicert.com`) |
| `AIO_MACOS_SIGN_IDENTITY`, `AIO_MACOS_NOTARY_PROFILE` | `ax release --aio` | The `codesign` identity and the `notarytool` keychain profile: aio-rs's own macOS `ax` is signed (hardened runtime) and notarized |
| `AIO_RELEASE_KEY` | `ax release --aio` | The key file that signs aio-rs's own `SHA256SUMS`, else `aio.release.key` in the keys folder (`ax keygen --aio`) |
| `AIO_FEEDBACK_DIR` | `ax feedback` | Where findings about aio-rs go (`<XDG data home>/aio/feedback`, shared with aio) |
| `AIO_DOWNLOAD` | `install.sh`, `install.ps1` | Where the installer downloads `ax` from, instead of the URL it was made with |
| `ANDROID_HOME`, `ANDROID_SDK_ROOT` | `ax build --android` | The Android SDK (else `~/Android/Sdk`) |
| `JAVA_HOME` | `ax build --android` | A JDK 17 or 21 for Gradle (its folder named `…-17…` or `…-21…`), else one in `/usr/lib/jvm` |

[Release](../build/release.md) has the signing steps; [targets](../build/targets.md) the builds.

## Set by aio-rs, not by hand

These pass a value between aio-rs's own processes. Setting one by hand has
no supported effect.

| Variable | Set by, for |
|---|---|
| `AIORT_APP_VERSION` | `ax build` (and `--android`, `--web`), as cargo builds the app: the version `aio::app_version!()` gives (`<[app] version>.<commit count>`). A plain `cargo build` without it gives the package version |
| `AIORT_APP_ID` | `ax build --web`: the app id the browser build keeps its storage under |
| `AIORT_CHANNEL` | `ax build` / `ax release --channel`: the channel stamped into the build (`aio::update::STAMPED_CHANNEL`) |
| `AIORT_RELEASED` | `ax release --aio`: an `ax` built for release, which knows no checkout of its own |
| `AIORT_DATA_CONTRACT` | `ax release`: asks the built app for the data it reads (the data gate), written to that file instead of starting |
| `AIORT_RELAUNCH_AFTER` | the updater: the new start waits for this pid to be gone |
| `AIORT_WATCH_FD` | `ax dev` and the test harness: the child stops gracefully when its parent goes, even on `kill -9` |
| `AIORT_DEV_CLOSED`, `AIORT_WINDOW_SIZE` | `ax dev` (`--size`): a debug build's dev window; ignored by a release build |
| `AIO_TEST_VIDEO`, `AIO_TEST_VIDEO_PACE` | `ax test --video` and `--pace`: each UI test recorded to a folder (one `.mp4` per test) or one `.mp4`/`.webm` file, each step on screen that many ms (`800`). [Testing](../testing/README.md) |

## Diagnostics and aio-rs's own tests

| Variable | Read by | Effect |
|---|---|---|
| `AIO_PRIVATE_TERMS` | `scripts/docs-check` | A file of words (one per line) no doc page may contain |
| `AIO_DENO_SRC` | aio-rs's `compat_wire` test | A copy of aio's `src`: the test then runs aio's own client against an aio-rs app (needs Deno); unset, it is skipped |

## Not aio-rs's

Read, never set: `HOME`, `USERPROFILE`, `USER`, `USERNAME`, `LOCALAPPDATA`,
`APPDATA`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_CONFIG_HOME`,
`XDG_BIN_HOME`, `XDG_RUNTIME_DIR`, `TMPDIR`, `PATH`, `DISPLAY`,
`WAYLAND_DISPLAY`, `XAUTHORITY`, `DBUS_SESSION_BUS_ADDRESS`, `GTK_THEME`
(a dark Linux desktop), `APPIMAGE`, `APPDIR`, `CARGO_TARGET_DIR`,
`CARGO_HOME`.

A child process the app starts loses `ELECTRON_RUN_AS_NODE`, `NODE_OPTIONS`,
`NODE_PATH`, an AppImage's own `APPIMAGE`, `APPDIR`, `ARGV0`, `OWD`, and
`PATH`/`LD_LIBRARY_PATH`/`LD_PRELOAD` entries inside an AppImage mount; a
child that is not an aio-rs app loses every `AIORT_*` variable, the key
included.

## aio variables aio-rs doesn't read

Known gaps or deliberate changes against aio's list:

| aio variable | In aio-rs |
|---|---|
| `AIO_PORT`, `AIO_DEFAULT_PORT` | none: `--port` per launch, `[server] listen` in `aio.toml` |
| `AIO_HOME`, `AIO_VERSIONS_DIR` | none: `ax` is one binary, no pinned framework versions |
| `AIO_TOKEN` (a per-user token for `am state --prod`) | none: `AIORT_TOKEN` is the app's own key |
| `AIO_INSTALL_ROOT` | none: `ax install` places apps itself |
| `AIO_TEST_ROOT`, `AIO_APPS_SANDBOX`, `AIO_TEST_PORT_SLICE` | none: each test gets its own temp sandbox (`aio::testing`) |
| `AIO_UPDATE_CHANNEL` | `AIORT_UPDATE_CHANNEL` |
| `AIO_MOVE_TO_APPLICATIONS` | `AIORT_MOVE_TO_APPLICATIONS` |
| `AIO_AM_DISPLAY` | `AIORT_DISPLAY` |
| `AIO_NO_OPEN` | `AIORT_NO_OPEN` |
| `AIO_ELECTRON_ARGS`, `AIO_ELECTRON_SANDBOX`, `AIO_CDP` | `AIORT_ELECTRON_ARGS`, `AIORT_SANDBOX`, `AIORT_CDP` (a port, debug builds only) |
| `ELECTRON_PATH`, `ELECTRON_MIRROR` | `AIORT_RUNTIME`, `AIORT_ELECTRON_MIRROR` (`AIO_SHARED_ELECTRON_DIR` is read as aio reads it: the store is one) |
| `AIO_DIALOG` | `AIORT_DIALOG` |
| `AIO_VIDEO`, `AIO_VIDEO_PACE` | `ax test --video`, `--pace`; no `AIO_VIDEO_SCHEME` (gap) |
| `AIO_BUILD_VERSION` | gap: `ax build` always derives the version from `[app] version` and git; a parent build cannot hand one down |
| `AIO_BUILD_COMMIT`, `AIO_PROBE_NONCE`, `AIO_BUILD_PROBE`, `AIO_BUILD_NOTES` | none: no runtime probe of the entry; the data contract is `AIORT_DATA_CONTRACT` |
| `AIO_PARENT_PID`, `AIO_PARENT_PID_FROM`, `AIO_LOCK_HANDOFF`, `AIO_APP_CWD`, `AIO_ELECTRON_RUNTIME` | none: `AIORT_WATCH_FD`, `AIORT_RELAUNCH_AFTER` and the home lock cover them |
| `AIO_SOCKET_TIMEOUT_MS` | gap: the window's request timeout is fixed |
| `AIO_DEV_SUPERVISED`, `AIO_DEV_RESTART_PORT`, `AIO_DEV_RESTART_SAVED`, `AIO_NO_DEV_RESTART` | none: `ax dev` hot-patches instead of restarting |
| `AIO_MAX_HEAP_MB` | none: no V8 heap in the app's process |
| `AIO_STDOUT_IS_LOG` | none: the log masks secrets everywhere |
| `AIO_ELECTRON_PROTOCOL`, `AIO_STRIP_CHROMIUM`, `AIO_SKIP_TRIM`, `AIO_WINDOWS_FAT_EXE` | none (no such build switches) |
| `AIO_AVD`, `AIO_ANDROID_SERIAL`, `AIO_PLAYWRIGHT_DIR` | gap: no Android emulator or WebKit test lane of `ax` |
| `AIO_DEBUG`, `AIO_DEV`, `AIO_AM_NO_DELEGATE` | none |
| `AIO_TEST_NAMES`, `AIO_TEST_WIRE`, `AIO_TEST_BROWSER_HOME`, `AIO_TEST_DISPLAY` | none: `TestUi` runs the real page in its own sandbox |

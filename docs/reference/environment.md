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
ones that move an app's data, key, port or profile when set (`AIORT_DATA`,
`AIORT_APPS_DIR`, `AIORT_PORT`, `AIORT_CONFIG`, `AIORT_PROFILE`, `AIORT_TOKEN`, `AIORT_SANDBOX`,
`AIO_APPS_DIR`, `AIO_PROFILE`, `AIO_DISCOVERY_PORT`, `AIO_FEEDBACK_DIR`;
the key as `(set)`, never its value).

Examples set a variable POSIX style, `AIORT_DATA=/tmp/notes-try ./notes`. In
PowerShell: `$env:AIORT_DATA = 'C:\Temp\notes-try'; .\notes.exe`.

## Running an app

Read by the app's process (`aio`, `aio-rs-shell`) at start.

| Variable | Values (default) | Effect |
|---|---|---|
| `AIORT_DATA` | a folder (unset) | All of the app's files under it: `data/`, `logs/`, `cache/`. Tests and portable installs use it; it wins over a ported app's own home rules too, so a sandbox never opens real aio data. Inside an `aio::testing` sandbox only a folder under the temp dir counts. [Where files live](../persistence/files.md) |
| `AIORT_APPS_DIR` | a folder (unset) | One root for every aio-rs app's home (aio's `AIO_APPS_DIR`): an app runs from `<folder>/<id>`, under `AIORT_DATA` and `[home] dir`; `ax remove --data` and `ax`'s other verbs follow it. Inside an `aio::testing` sandbox only a folder under the temp dir counts |
| `AIORT_INSTALL_ROOT` | a folder (unset: `aio-rt/install` in the data home) | Where a Linux bundle's `install.sh` puts the app (`<folder>/<id>/versions/…`, aio's `AIO_INSTALL_ROOT`); `uninstall.sh`, `ax installed` and `ax remove` look there. The installed app's updates find their folder by where the binary runs from, so the variable need not be set when it runs |
| `AIORT_PROFILE` | a name or a path (unset) | A second home beside the default one, `<home>-<name>`: the variable form of `--profile`; the flag wins |
| `AIORT_TOKEN` | a long random string (unset) | The app's key (sync, the local door, `ax`), instead of `data/sync.token`. Under 16 characters or 80 bits it is used and warned about. Never passed on to a child that is not an aio-rs app. [Security](../auth/README.md) |
| `AIORT_PORT` | a port, `0` to `65535` (unset) | The port a serving app listens on, over `[server] listen`'s: the variable form of `--port` (aio's `AIO_PORT`); the flag wins. The boot report names it (`listens on 127.0.0.1:9100 (…; AIORT_PORT)`) |
| `AIORT_UPDATE_CHANNEL` | a channel name (unset) | Follow this update channel. Order: the launch flag `--channel=<c>`, the variable, then the person's own choice (`set_channel`), then the channel the build was stamped with, then `[updates] channel`. [Release](../build/release.md) |
| `AIORT_CONFIG` | anything | Refused: the app stops with an error naming the per-launch flags and variables to use instead. An app reads only the `aio.toml` it was built with |
| `AIORT_LOG` | `trace`, `debug`, `info`, `warn`, `error` (unset) | The log's lowest level for this run, over `[log] level`; `ax dev --verbose` sets `debug`. Another value is warned about and `[log] level` holds |
| `AIORT_ISOLATE` | cell ids, comma separated (unset) | Only those cells start (aio's `isolate`; an app that starts its cells as one `aio::Cells` list); the others stay disabled, their calls refused and their schedules held, said in the log. An id the app has no cell for refuses the boot, naming its cells |
| `AIO_PROFILE` | a name or a path | aio's profile variable, read only by a ported app (`[app] port_of`), after `--profile` and `AIORT_PROFILE` |
| `AIO_APPS_DIR` | a folder | aio's root of every app's data, read only by a ported app: its home is `<AIO_APPS_DIR>/<aio app>`, as in aio (any other app: `AIORT_APPS_DIR`) |
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
| `AIORT_CDP` | a port (unset) | An `ax dev` build's window opens the DevTools protocol on 127.0.0.1 at that port. Another build (a release, a plain `cargo build`) never reads it; one whose app says `Window { devtools: true }` opens one on the launch's typed `--cdp=<port>`. `ax shot`, `ax eval`, `ax surface` and `ax trigger` attach to it when `--cdp` is not given |
| `AIORT_DIALOG` | `none`, `browser` (unset) | The first-run and error windows (the runtime download, Retry or Quit): `none` shows none (the log has it all); `browser` uses a browser page instead of zenity or yad on Linux |
| `AIORT_TITLE` | a title (unset) | The window's title for this run instead of the app's (aio's `--title=`); `ax dev --title` sets it |
| `AIORT_NO_OPEN` | non-empty (unset) | The app hands nothing to the person's browser or file manager: each open is refused, naming what it would have opened. `ax dev` sets it for a caller with no terminal; a test sandbox has it always |

## `ax` and building

Read by `ax` (the `aio-rs-cli` binary) and the install scripts.

| Variable | Read by | Effect |
|---|---|---|
| `AIORT_DISPLAY` | `ax dev`, `ax start` | Where the window goes when `--display` is not given: `auto` (default: no terminal, a nested display), `isolated`, `current` or `:N` |
| `AIORT_UI_ROOTS` | `ax ui` | More folders to find projects in (stopped apps), `:`-separated (`;` on Windows), beside `~/aio-apps`, the home folder, the current folder and the running apps' folders. [`ax ui`](../clients/ax.md#ax-ui-the-visual-app-manager) |
| `AIORT_NO_SPLASH` | `ax dev` | `1`: no splash window while the first build runs (its progress and errors are on the terminal either way); `0` or empty: as unset |
| `AIORT_NO_WARM` | `ax create`, `ax setup` | `1`: no build in the background (of the new app, as `--no-warm`; of aio-rs after `ax setup`); `0` or empty: as unset |
| `AIORT_PREBUILT_URL` | `ax dev` | where the first build fetches prebuilt registry and git libraries (`<url>/<key>/SHA256SUMS`, `<key>.zip`); `https://` only; unset: none (a prototype) |
| `AIORT_PREBUILT_KEY` | `ax dev` | the `ed25519:` public key `SHA256SUMS.sig` must verify against. Required: without it nothing is fetched |
| `AIORT_PREBUILT_SIGN` | `ax dev --warm --pack` | the key file (`ax keygen --aio`) `SHA256SUMS` is signed with |
| `AIORT_TARGET_DIR` | every `ax` verb that runs cargo | ax's build folder, instead of the shared one per toolchain (`<cache home>/target/<toolchain>`, the toolchain of the workspace's root, which ax then sets as `RUSTUP_TOOLCHAIN` for its cargo); wins over `CARGO_TARGET_DIR` |
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
| `ANDROID_HOME`, `ANDROID_SDK_ROOT` | `ax build --android`, `ax dev --android`, `ax install --android` | The Android SDK, or its parent (else the OS's default: `~/Android/Sdk`, `~/Library/Android/sdk`, `%LOCALAPPDATA%\Android\Sdk`) |
| `ANDROID_SERIAL` | `ax install --android`, `ax dev --android`, `ax test --android` | adb's own: the device to use when `--device` names none |
| `ANDROID_USER_HOME` | `ax build --android` | Where Android's debug key is (else `~/.android`): a port signs with it, as aio's APK is |
| `AIO_AVD` | `ax dev --android`, `ax test --android` | The emulator (AVD) booted when no device is attached (else the first) |
| `AIO_AVD_PORT` | `ax dev --android`, `ax test --android` | The booted emulator's console port (`emulator-<port>`; else the emulator's first free one) |
| `JAVA_HOME` | `ax build --android` | The JDK for Gradle when its `javac` says 17 to 23, else the newest up to 21 in the OS's JDK folders (Android Studio's own too) |

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
| `AIORT_DEV_CLOSED`, `AIORT_WINDOW_SIZE` | `ax dev` (`--size`): its window's tooling (aio-shell's `dev` feature, which only ax dev's builds have, and `ax start --cdp`'s); ignored by any other build, `ax start`'s too |
| `AIORT_BOOT` | `ax dev`: `brief`, the app's boot report in one line on its terminal (all of it in the log) |
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
| `AIO_PORT` | `AIORT_PORT` (under `--port`, over `[server] listen`) |
| `AIO_DEFAULT_PORT` | none: `[server] listen` in `aio.toml` is the default port, and an app with none gets a free one |
| `AIO_HOME`, `AIO_VERSIONS_DIR` | none: `ax` is one binary, no pinned framework versions |
| `AIO_TOKEN` (a per-user token for `am state --prod`) | none: `AIORT_TOKEN` is the app's own key |
| `AIO_INSTALL_ROOT` | `AIORT_INSTALL_ROOT` |
| `AIO_TEST_ROOT`, `AIO_APPS_SANDBOX`, `AIO_TEST_PORT_SLICE` | none: each test gets its own temp sandbox (`aio::testing`) |
| `AIO_UPDATE_CHANNEL` | `AIORT_UPDATE_CHANNEL` |
| `AIO_MOVE_TO_APPLICATIONS` | `AIORT_MOVE_TO_APPLICATIONS` |
| `AIO_AM_DISPLAY` | `AIORT_DISPLAY` |
| `AIO_NO_OPEN` | `AIORT_NO_OPEN` |
| `AIO_ELECTRON_ARGS`, `AIO_ELECTRON_SANDBOX`, `AIO_CDP` | `AIORT_ELECTRON_ARGS`, `AIORT_SANDBOX`, `AIORT_CDP` (a port, `ax dev` builds only) |
| `ELECTRON_PATH`, `ELECTRON_MIRROR` | `AIORT_RUNTIME`, `AIORT_ELECTRON_MIRROR` (`AIO_SHARED_ELECTRON_DIR` is read as aio reads it: the store is one) |
| `AIO_DIALOG` | `AIORT_DIALOG` |
| `AIO_VIDEO`, `AIO_VIDEO_PACE` | `ax test --video`, `--pace`; no `AIO_VIDEO_SCHEME` (gap) |
| `AIO_BUILD_VERSION` | `ax build --app-version v` and `ax release --app-version v`: a flag, not a variable |
| `AIO_BUILD_COMMIT`, `AIO_PROBE_NONCE`, `AIO_BUILD_PROBE`, `AIO_BUILD_NOTES` | none: no runtime probe of the entry; the data contract is `AIORT_DATA_CONTRACT` |
| `AIO_PARENT_PID`, `AIO_PARENT_PID_FROM`, `AIO_LOCK_HANDOFF`, `AIO_APP_CWD`, `AIO_ELECTRON_RUNTIME` | none: `AIORT_WATCH_FD`, `AIORT_RELAUNCH_AFTER` and the home lock cover them |
| `AIO_SOCKET_TIMEOUT_MS` | gap: the window's request timeout is fixed |
| `AIO_DEV_SUPERVISED`, `AIO_DEV_RESTART_PORT`, `AIO_DEV_RESTART_SAVED`, `AIO_NO_DEV_RESTART` | none: `ax dev` hot-patches instead of restarting |
| `AIO_MAX_HEAP_MB` | none: no V8 heap in the app's process |
| `AIO_STDOUT_IS_LOG` | none: the log masks secrets everywhere |
| `AIO_STRIP_CHROMIUM` | `[build] chromium_extras = "strip"` in `aio.toml` |
| `AIO_ELECTRON_PROTOCOL`, `AIO_SKIP_TRIM`, `AIO_WINDOWS_FAT_EXE` | none (no such build switches) |
| `AIO_ANDROID_SERIAL`, `AIO_PLAYWRIGHT_DIR` | `ANDROID_SERIAL` (adb's own) names the device of `ax test --android`; no WebKit test lane |
| `AIO_DEBUG`, `AIO_DEV`, `AIO_AM_NO_DELEGATE` | none |
| `AIO_TEST_NAMES`, `AIO_TEST_WIRE`, `AIO_TEST_BROWSER_HOME`, `AIO_TEST_DISPLAY` | none: `TestUi` runs the real page in its own sandbox |

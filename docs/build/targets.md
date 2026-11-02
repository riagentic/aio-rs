# Targets

One app, one codebase, several ways to run: a desktop window, an Android
app, a page in a browser, a headless service, a command-line tool, or a
thin client of a server somewhere else. The cells and the views are the
same everywhere; what changes is the launcher, where the code runs and
where the data lives. This page is every target, every `[build]` key,
what each bundle holds and how the targets differ.

| Target | Runs as | Make it | While developing |
|---|---|---|---|
| Desktop (Linux, Windows, macOS) | the app's process, its window drawn by the shared Electron runtime | `ax build <app>` | `ax dev <app>` |
| Android | the app's library in a small activity, drawn by the system WebView | `ax build <app> --android` | `ax dev <app> --android` |
| Web | the views and cells as wasm in the browser's page (an installable PWA) | `ax build <app> --web` | `ax dev <app> --web` |
| Browser UI | a client-server app that shows its page in the system browser | `[app] ui = "browser"` | `ax dev <app>` |
| Service | cells served to clients, no window | `ax create --template service`, kind `cli` | `ax dev <app>` |
| CLI | one command on a saved cell, then out | `ax create --template cli`, kind `cli` | `ax dev <app> -- <args>` |
| Thin client | a window (or a phone app) on cells another process serves | `[app] mode = "client"` | `ax dev <app>` |

What a machine needs for each: [setup](../basics/setup.md). Releasing and
updating each: [release](release.md).

## The app's role: `[app] mode`

| `mode` | Cells live | Listens | A window |
|---|---|---|---|
| `standalone` (default) | in the app's own process | nothing | yes |
| `client-server` | here, and served (`[server] listen`) | yes | yes (or the browser, `ui = "browser"`) |
| `client` | on a server (`[client] server`, or one paired with `ax pair`) | nothing | yes |
| `server` | here, served | yes | no |
| `cli` | in the tool's process, for one command | nothing | no |

`aio::Source::for_app(&cfg, &dirs)` opens the right one for the mode;
a mistake is a compile error naming the key:

```rust
use aio::config::Config;

# fn main() {
let e = Config::parse("[app]\nid = \"team\"\nmode = \"client-server\"\n").unwrap_err();
assert!(e.contains("[server] listen"), "{e}");
let e = Config::parse("[app]\nid = \"team\"\nui = \"browser\"\n").unwrap_err();
assert!(e.contains("client-server"), "a browser UI needs its own server: {e}");
let ok = Config::parse("[app]\nid = \"team\"\nmode = \"client\"\n\n[client]\nserver = \"192.0.2.10:8100\"\n\n[capabilities]\nnetwork = [\"192.0.2.10\"]\n");
assert!(ok.is_ok(), "{ok:?}");
# }
```

## `ax build`: what, for which machines

```sh
ax build notes                          # [build] targets, else this OS's bundle
ax build notes --list                   # the kinds, and the app's targets
ax build notes --targets desktop,deb    # some of them (labels or kinds)
ax build notes --platforms linux,windows
ax build notes --deb                    # the bundle and its .deb (--appimage too; --windows, --android: that kind alone)
ax build notes --web                    # the web app instead, in dist/notes-web/
ax build notes --sizes                  # the release binary against its budget, packed nowhere
ax build notes --smoke                  # then start each Linux bundle headless and stop it
```

Everything lands in the workspace's `dist/`. One target failing doesn't
stop the rest: the summary names each failed one and the command that
builds it alone. Every build also writes `dist/<id>.manifest.json`: what
was built, from which commit, each target's files, and a warning when the
same version was built before from other sources or when the tree was
dirty (a dirty build is never a release).

### The kinds

| Kind | Makes | Where it builds |
|---|---|---|
| `desktop` | this OS's bundle: Linux `.tar.gz`, macOS zipped `.app`, Windows one-click `.exe` and a `.zip` | each of `platforms` |
| `windows` | the Windows `.exe` and `.zip`, cross-built from Linux (`--windows`) | Linux, with MinGW |
| `deb` | a Debian package (`--deb`; needs `[app] maintainer`) | Linux |
| `appimage` | one portable Linux file (`--appimage`) | Linux |
| `dmg` | a macOS disk image to drag into Applications | a Mac (or one over ssh) |
| `cli` | a headless binary on `PATH`, no menu entry; a server's also gets its supervisor file | each of `platforms` |
| `android` | signed APKs, one per CPU, and an `.aab` when asked (`--android`) | Linux, macOS, Windows |
| `web` | the installable web app in `dist/<id>-web/` (`--web` alone) | anywhere with the `wasm32` target |

Nothing declared: `desktop`, or `cli` for a `server` or `cli` app.

### Every `[build]` key

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
runtime = "shared"
chromium_extras = "keep"
css = "tailwindcss -i src/app.css -o assets/style.css"

[build.assets]
"/media" = "media"

[build.splash]
image = "assets/splash@2x.png"
text = "{title}"
min_ms = 400
max_ms = 15000
background = ["#f6f7f9", "#14161a"]
font = "sans-serif"
font_size = 14
bold = false
fade_ms = 200

[build.macos]
bundle_id = "org.example.notes"
host = "builder@mac.example.org"

[build.android]
application_id = "org.example.notes"
keystore = "keys/release.jks"
key_alias = "notes"
aab = true
schemes = ["notes"]
camera = false
color_scheme = "system"
universal = false
strict = false
```

| Key | Default | Means |
|---|---|---|
| `targets` | `["desktop"]` (`["cli"]` for a server or CLI) | kinds, or `{ name, kind, bin }`: a label, its kind (else the name), the workspace binary it builds (else the app) |
| `platforms` | this machine | each desktop and `cli` target for each of `linux`, `linux-arm64`, `windows`, `windows-arm64`, `macos`, `macos-arm64`, `host` |
| `budget_mb` | 10 (a server: 6) | the release binary's size budget; a build above it fails and names the crates that take the space |
| `smoke` | false | every build then starts each bundle it can (as `--smoke`) |
| `runtime` | `shared` | `shared`: one Electron per machine, fetched at the first start; `bundled`: inside the package, for offline machines |
| `chromium_extras` | `keep` | a bundled runtime's software Vulkan and DirectX shader compilers; `strip` drops them (smaller, draws worse where no GPU driver works) |
| `css` | none | a CSS toolchain run before every build and on every change in `ax dev`: a command split on spaces, or an argv list; never through a shell |
| `[build.splash]` | none | [the splash](#the-splash) |
| `[build.assets]` | none | folders beside `aio.toml` built into the app by URL prefix, answered at the same path in the window, the app's server, the web build and the APK ([build](README.md#build-ax-build)) |
| `[build.macos]` | | [macOS](#macos) |
| `[build.android]` | | [Android](#android) |

The app's name, version and icon come from `[app]` and the app's folder:
`[app] name` is the window title, the menu entry and the bundle's name;
`[app] id` is the binary, the package name and the data's home;
`assets/icon.png` beside `aio.toml` (a square PNG, 512 px or more) is the
icon of every package and, built in by `aio::config!()`, of the running
window; `assets/icon.ico` is the Windows exe's as it is. None: a
monogram of the name in the app's hue, drawn at each size, 512 px plus an
SVG for Linux. `ax build` says when an icon can't be used: not square,
an `icon.svg` (not read), an `icon.png` outside `assets/`. Versions:
[build](README.md#versions).

### One repo, several apps

A labelled target with `bin` builds another binary of the workspace, with
its own `aio.toml`, its own id and its own data:

```toml
[build]
targets = [
  "desktop",
  { name = "pro", kind = "desktop", bin = "notes-pro" },
  { name = "admin", kind = "cli", bin = "notes-admin" },
]
```

`ax build notes --targets pro` builds one; `ax start pro` and `ax dev
pro` find the binary by its label. Two targets that would write the same
file are refused; two apps that show the same name in the menu are
warned about.

```rust
# fn main() {
let cfg = aio::config::Config::parse(
    "[app]\nid = \"notes\"\n\n[build]\ntargets = [\"desktop\", { name = \"admin\", kind = \"cli\", bin = \"notes-admin\" }]\n",
)
.unwrap();
let admin = &cfg.build.targets[1];
assert_eq!((admin.name.as_str(), admin.kind.as_deref(), admin.bin.as_deref()), ("admin", Some("cli"), Some("notes-admin")));
assert_eq!(cfg.mac_bundle_id(), "aio.rt.notes", "unset: aio.rt.<id>");
# }
```

### Other operating systems: `platforms`

Each platform is a Rust target; a cross build needs it installed and a
linker for it, and what is missing is said before cargo runs, with the
command that adds it:

| Name | Rust target | Linker from Linux |
|---|---|---|
| `linux` | `x86_64-unknown-linux-gnu` | `x86_64-linux-gnu-gcc` |
| `linux-arm64` | `aarch64-unknown-linux-gnu` | `aarch64-linux-gnu-gcc` (`gcc-aarch64-linux-gnu`) |
| `windows` | `x86_64-pc-windows-gnu` | `x86_64-w64-mingw32-gcc` (`gcc-mingw-w64-x86-64`) |
| `windows-arm64` | `aarch64-pc-windows-gnullvm` | `aarch64-w64-mingw32-clang` (llvm-mingw) |
| `macos` | `x86_64-apple-darwin` | none: built on a Mac |
| `macos-arm64` | `aarch64-apple-darwin` | none: built on a Mac |
| `host` | this machine's | |

A macOS binary links against Apple's SDK, so it is built on a Mac. Away
from one, `[build.macos] host` (or `AIO_MACOS_SSH`, which it outranks)
names one over ssh (`[ssh options] [user@]host`): the sources go there,
it builds (and smoke-tests with `--smoke`) there, the files come back
into `dist/`.

### Size: `--sizes` and the budget

`ax build notes --sizes` builds the release binary with ax's link flags
and shows it against its budget, and which crates take its space; it
packs nothing. A plain `cargo build --release` lacks those flags and is
a few percent bigger: don't size with it. A build over budget is never
packed. Raise `budget_mb` for an app with heavy dependencies, on purpose.

### Start what you built: `--smoke`

`ax build notes --smoke` (or `[build] smoke = true`) unpacks each bundle
into a throwaway sandbox and starts it as a user would, on a display no
one sees (Xvfb, else the runtime's own headless mode; never the person's
screen) with a private D-Bus. A window app must open its window, stop on
SIGTERM, leave nothing running and log no panic, report or degraded part;
a server must boot on a free port; a CLI must answer `--version` with its
id. `ax smoke <artifact>` does one file and reports its boot time and its
window's memory.

- Linux bundles, `.deb` and AppImage: on this machine.
- A Windows `.zip`: on Windows, this one or the one `AIO_WINDOWS_SSH`
  names; it is stopped through its door (`<app> --quit`).
- A macOS `.zip`: on the Mac that built it; its window shows on that
  Mac's screen until it is asked to stop.

## Desktop

```rust,ignore
// ax create's template (app/src/main.rs), compiled by ax's own tests:
let cfg = aio::config!();
let Start::Run(dirs) = aio_shell::start(&cfg, aio::app_version!())? else { return Ok(()) };
_ = app::SOURCE.set(aio::Source::for_app(&cfg, &dirs)?);
aio_shell::run(app::App, aio_shell::Window { width: 900, height: 640, ..aio_shell::Window::of(&cfg) }, &dirs)
```

- `aio_shell::start` takes the app's launch flags ([dev](dev.md#the-apps-own-launch-flags))
  and claims its data; a second launch hands its arguments to the running
  copy, which comes to the front. `Source::for_app` opens the cell on this
  process's store, or a synced copy of `[client] server`'s; `aio_shell::run`
  opens the window ([views](../ui/views.md#the-window)).
- The views run in the app's Rust process; the window only draws them.
  Every crate, file, thread and the SQL store work in a view's code.
- `Window` holds the title, size, icon, background before the first
  paint, `lang`, `dir`, viewport, tray, extra `<head>` markup, embedded
  sites (`web`), page permissions (`"clipboard-read"`, `"media"`…), the
  beforeunload prompt and the window's own keys. The window's size and
  place are saved per app and put back, on the screen it was on.

### The shared runtime

The window is drawn by a pinned Electron (the same version aio pins),
shared by every aio-rs and aio app on the machine: one copy per version, in
aio's Electron store (`~/.local/share/aio/electron/`, `~/Library/Application
Support/aio/electron/`, `%LOCALAPPDATA%\aio\electron\`).

- Fetched at an app's first start (or by `ax setup`, `ax runtime --fetch`)
  from Electron's releases, checked against the SHA-256 the build pins, unpacked
  beside its place and renamed in, so a half download is never used.
  `AIORT_ELECTRON_MIRROR` names another source; `AIORT_RUNTIME` a runtime
  folder to use as is.
- It is aio's, never the app's to update: an update of the app that needs
  a newer runtime fetches it before it swaps.
- `ax prune` lists what no installed app names and nothing started from in
  30 days (`--yes` deletes it); `ax runtime` says where it is.
- `[build] runtime = "bundled"`: the runtime goes into the package beside
  the binary, trimmed (English locales only, and with `chromium_extras =
  "strip"` the GPU extras), for machines without a network.
- The GPU: drawn on it, and in software where it failed before;
  `AIORT_GPU=software` or `hardware` decides.

### Linux

| File | Install | Remove |
|---|---|---|
| `notes-1.4.7-linux-x64.tar.gz` | `tar -xzf`, then `./install.sh`: for this user, no root | `uninstall.sh` (keeps the data; `--data` removes it too) |
| `notes_1.4.7_amd64.deb` (`--deb`) | `sudo apt install ./notes_1.4.7_amd64.deb` | `sudo apt remove notes` |
| `notes-1.4.7-x86_64.AppImage` (`--appimage`) | run it as is, or `ax install notes --appimage` | delete it, or `ax remove notes` |

- `install.sh` puts the binary in
  `$XDG_DATA_HOME/aio-rt/install/<id>/versions/<version>/` with `current`
  pointing at it (how updates swap versions), links `~/.local/bin/<id>`,
  writes the menu entry and its icon, and `installed.json` (what and from
  where; `ax installed` reads it). The menu entry's window class is the
  app id, so the taskbar groups its window under its icon.
- A `.deb` installs system-wide (`/usr/lib/<id>`, `/usr/bin/<id>`, the
  menu entry) and depends on the libraries Electron needs (GTK 3, NSS,
  GBM, ALSA…). The package manager updates it: the app's own updater stays
  off. The package's version sorts like the app's (`1.4.0-rc.1` is
  `1.4.0~rc.1`, before `1.4.0`).
- An AppImage carries the pinned AppImage runtime (checked by its
  SHA-256) and the app; the Electron runtime is still shared. An app run
  from one updates by replacing the file (`ax release --appimage`).
- `ax install <app> --appimage` (aio's installer): the AppImage copied to
  `$XDG_DATA_HOME/aio-rt/install/<id>-appimage/<id>.AppImage` (where its
  updates go), its icon, and a menu entry whose `Exec` makes a private
  `TMPDIR` (`$XDG_CACHE_HOME/aio-rt/<id>/tmp`, mode 0700) before the
  AppImage runs: the AppImage runtime unpacks into `TMPDIR` before any of
  the app can run, and `/tmp` is shared. `ax installed` lists it as
  `<id> (AppImage)`, `ax remove` takes it away.
- None of them carries the runtime unless `runtime = "bundled"`.
- Chromium's sandbox: where unprivileged user namespaces are restricted
  (Ubuntu 23.10+, some containers), the window starts with `--no-sandbox`
  and says so; `AIORT_SANDBOX=1` refuses instead
  ([troubleshooting](../debugging/troubleshooting.md#linux)).

### Windows

| File | Is |
|---|---|
| `notes-1.4.7-windows-x64.exe` | the one-click download: double-clicked, it installs itself and starts |
| `notes-1.4.7-windows-x64.zip` | `notes.exe` alone: what updates fetch |

- The exe installs for this user, no admin rights: into
  `%LOCALAPPDATA%\Programs\<id>`, a Start menu entry, and an entry in
  Settings › Apps whose Uninstall runs `notes.exe --uninstall` (the data
  stays). `notes.exe --install` does the same from a script.
- Opened again, or once the installed app has updated past it, the
  download only starts the installed copy: an old download never puts its
  version over a newer install. A browser's ` (1)` in the name is fine.
- The exe wears the app's icon. With `runtime = "bundled"` the one-click
  exe carries the runtime appended and puts it beside the installed exe.
- Cross-built from Linux (`--windows`, or `windows` in `platforms`) with
  MinGW, or built on Windows. The bundles are not OS-signed yet:
  SmartScreen warns on the first open ([release](release.md#signing-for-the-os)).

### macOS

| File | Install |
|---|---|
| `notes-1.4.7-macos-arm64.zip` | unzip, drag `Notes.app` to Applications |
| `notes-1.4.7-macos-arm64.dmg` (kind `dmg`) | open, drag `Notes.app` onto the Applications link |

- The `.app` holds the binary, its icon (an `.icns` made in Rust with every
  slot from 16 to 1024 px: `assets/icon.png` scaled, or the monogram drawn),
  `Info.plist` with `[build.macos] bundle_id` (else `aio.rt.<id>`; it is
  the app's identity, never change it) and macOS 11 as the minimum. It is
  signed ad hoc (Apple silicon runs nothing unsigned), not with a
  Developer ID, and not notarized: Gatekeeper asks on the first open of a
  downloaded copy (Control-click → Open once, or
  `xattr -dr com.apple.quarantine Notes.app`).
- The bundle only launches the app; the Dock entry is the window's.
- Opened from its disk image or from Downloads, the app runs where an
  update can't replace it (a read-only image, or the copy macOS makes of
  a quarantined app). Once its window is up it offers, once, to move
  itself into Applications and restart from there; "Not Now" is
  remembered. `AIORT_MOVE_TO_APPLICATIONS=never` turns the offer off,
  `move` moves without asking (an unattended install). An update that
  can't replace the app is refused before it downloads, saying why.
- Built on a Mac (`codesign`, `sips`, `hdiutil`, `ditto`), or on one over
  ssh ([platforms](#other-operating-systems-platforms)).

### The splash

```toml
[build.splash]
image = "assets/splash@2x.png"   # a PNG in the app's folder; …@2x.png is drawn at half its size
text = "{title}"                 # under the image; {title} is the app's name
min_ms = 400                     # once shown, at least this long
max_ms = 15000                   # no window by then: it goes anyway
background = ["#f6f7f9", "#14161a"]  # light, dark; unset: the app's own colours
font = "serif"                   # Android: the text's font family; unset: the system's
font_size = 16                   # Android: points; unset: 14
bold = true                      # Android: the text in bold
fade_ms = 300                    # Android: its fade out; unset: 200
```

Shown from the app's start until its window has loaded, on the desktop,
the web build and Android (there the starting window is already its
colour). The image is built into the app.

## Android

```rust,ignore
// ax create's template (app/src/lib.rs), compiled by ax's own tests:
#![cfg(target_os = "android")]
aio_shell::android_app!(aio_shell::android::App {
    config: || aio::config!(),
    version: aio::app_version!(),
    root: app::App,
    window: aio_shell::Window::default(),
    setup: |cfg, dirs| {
        _ = app::SOURCE.set(aio::Source::for_app(cfg, dirs)?);
        Ok(())
    },
});
```

The app's library is built as a `cdylib` for each CPU and loaded by a
small Java activity; the view tree lives in Rust and the system WebView
only draws it. DOM edits and events cross a message port: the page has no
network, loads no file and sees nothing else of the app. Its origin is a
reserved `https://app.invalid`.

### What it needs, what it makes

- On Linux, macOS or Windows: the Android SDK (`ANDROID_HOME`,
  `ANDROID_SDK_ROOT`, each the SDK or its parent, else the OS's default:
  `~/Android/Sdk`, `~/Library/Android/sdk`, `%LOCALAPPDATA%\Android\Sdk`)
  with an NDK and build-tools; a JDK 17 to 23 (each candidate's `javac`
  asked its version, so a JRE is never taken: `JAVA_HOME` when it fits,
  else the newest up to 21 in the OS's JDK folders, Android Studio's own
  included); any Gradle 8.9 or newer (9 too) on `PATH` or in
  `~/.gradle`, which only writes the wrapper: the build runs Gradle 8.14.3,
  pinned; and the Rust targets `aarch64-linux-android`,
  `armv7-linux-androideabi` and `x86_64-linux-android`. `ax setup --check`
  names what is missing.
- `ax build notes --android`: `dist/<id>-<version>-android-arm64-v8a.apk`,
  `…-armeabi-v7a.apk` (32-bit ARM, as aio ships it) and `…-x86_64.apk`,
  each zipaligned, signed and within the 10 MB budget; with
  `universal = true` also `…-android-universal.apk` (every CPU in one file,
  to share; `ax install` takes it when there is none for the phone's CPU);
  with `aab = true` also `dist/<id>-<version>-android.aab` for Play.
  minSdk 29 (Android 10, the oldest whose WebView still gets updates, as
  aio), targetSdk 35.
- `ax build notes --android --debug`: debug APKs instead
  (`…-android-<abi>-debug.apk`), debuggable and signed with Android's debug
  key: the page shows in `chrome://inspect`. Install one with
  `ax install notes --android --apk <file>`.
- The version code is aio's: `major·10⁸ + minor·10⁶ + patch`, so `1.4.7`
  is 104000007 and a port's APK installs over the aio APK of the same
  version number (aio's third number is its build count). Out of range
  (major over 20, minor over 99, patch over 999999) is refused, never
  wrapped. A prerelease shares its release's code, so never ship both to
  the same users.

### Identity and signing

```toml
[build.android]
application_id = "org.example.notes"  # its permanent identity; unset: aio.rt.<id> (a port: aio's)
keystore = "keys/release.jks"         # relative: the app's folder; unset: aio's key for the app
key_alias = "notes"
```

- Android updates an installed app only from an APK with the same
  application id signed by the same key. Never change either once
  users have it.
- No keystore named: a key is made on first use in
  `~/.config/aio-rt/keys/<id>.android.p12`. Keep it (back it up): a lost
  key means users must uninstall to get the next version. A port signs
  with aio's key instead (below).
- The password is never in `aio.toml`: the build reads
  `AIO_ANDROID_KEYSTORE_PASSWORD` (and `AIO_ANDROID_KEY_PASSWORD` when
  the key has its own). In CI, `AIO_ANDROID_KEYSTORE` and
  `AIO_ANDROID_KEY_ALIAS` override the file's. A named keystore that
  can't be used is refused before anything builds.

```rust
# fn main() {
let cfg = aio::config::Config::parse(
    "[app]\nid = \"notes\"\n\n[build.android]\napplication_id = \"org.example.notes\"\nschemes = [\"notes\"]\n",
)
.unwrap();
assert_eq!(cfg.build.android.schemes, ["notes"]);
let bad = |toml: &str| aio::config::Config::parse(toml).unwrap_err();
assert!(bad("[app]\nid = \"notes\"\n\n[build.android]\nschemes = [\"https\"]\n").contains("the system's"));
assert!(bad("[app]\nid = \"notes\"\n\n[build.android]\napplication_id = \"notes\"\n").contains("not an Android package name"));
# }
```

### Over aio's APK: a port's upgrade

A port (`[app] port_of = "notes"`, [migration](../migration/README.md))
installs over the aio APK of that app, its data with it:

- Its application id is aio's, `app.aio.<port_of without non-[a-z0-9]>`
  (a declared `application_id` wins: give the aio app's own when it had
  one, or when its APK's name was not its id).
- Its version code is aio's formula (above): give it a version above
  the aio build's (`1.2.346` over aio's `1.2.345`).
- No keystore named: it signs with this machine's Android debug key
  (`~/.android/debug.keystore`; `ANDROID_USER_HOME`, or `$XDG_CONFIG_HOME/.android` when Gradle put it there), the key
  aio's default build signs with; the build says so. An aio release that
  was signed with your keystore: name the same one (`keystore`, or
  `AIO_ANDROID_KEYSTORE`).
- At its first start the aio APK's state (`filesDir/aio-store`, key
  `aio:<port_of>`) becomes the port's store in aio's layout, which its
  cells start from as from aio's desktop `state.db`; once (only while the
  port has no store), and aio's files are left as they were.

### The dev loop

```sh
ax dev notes --android                 # the attached emulator (before a phone), else AIO_AVD booted
ax dev notes --android --device R58N…  # that one (or ANDROID_SERIAL)
ax dev notes --android --native        # the app itself, rebuilt on each save
```

- An app with a web build (`ax create`'s has one) runs as its dev page, as
  aio's `dev:android`: a thin dev APK (no library: Gradle only, no NDK)
  is built once and installed once beside the real app
  (`<application id>.dev`, "Notes (dev)"); `ax dev --web` serves the page
  on this machine, `adb reverse` brings it to the device's
  `http://127.0.0.1:<port>`, and each edit rebuilds it there and reloads
  it on the device. The page is the web build's: its cells run in the
  page, not in a native library. Ctrl-C removes ax's reverse (only its
  own); one a killed session left is removed at the next start.
- `--native`, or an app without a web build: the app's own library as a
  debug dev copy beside the real app (`.dev`), built for that device's
  CPU only, installed and started again on every save.
- No device attached: `AIO_AVD` (else the first AVD) is booted, on
  console port `AIO_AVD_PORT` when set, without a window where there is no
  display, and waited for (up to 180 s); it is left running for the next
  run.
- The app's log, its page's console and a crash reach the terminal as
  `[device] …` (`adb logcat -s aio AndroidRuntime chromium`), a page
  error with its file and line.

### The emulator lane

```sh
ax test notes --android                # AIO_AVD booted when none is attached
ax test notes --android --device emulator-5556
```

The app's release APK for the device's CPU is built, installed and
started; the lane passes once the window is ready within 60 s and the app
still runs 3 s later, and fails with the device's own lines on a crash.

### How it behaves on a phone

| Concern | What aio-rs does |
|---|---|
| The OS kills the app | every commit is on disk before the call returns (`durability = full`, always on a phone); the next start comes back to it |
| The page's renderer is lost (memory) | the WebView is rebuilt and the page reloads; what was committed is in the store. A second loss within 10 s stops the app instead of looping |
| Back | the views' handlers first ([routing](../ui/routing.md#back)); none takes it: the page's history, then the app leaves |
| Deep links | `schemes = ["notes"]`: a `notes://…` link opens the app (one copy, `singleTask`) and reaches it (`aio::ui::links`; a `router!` goes there). Lower case; none of the system's (`https`, `content`, `file`…) |
| The camera | `camera = true` declares it (optional hardware: it still installs without one); the window must allow `"media"`; Android asks once. Only `getUserMedia({ video: true })` alone: anything else asked with it is denied |
| `<input type="file">` | the system's picker; the app gets exactly what the user picks, no permission |
| A download the page starts | a link on the network goes to Android's DownloadManager (into Downloads, with its notification); a `data:` one is saved where the user picks; a `blob:` one is logged (the page has no network to read it: the app saves files, `aio_shell::page::save`) |
| The phone's images | `aio::ui::local_file_url(path)` (images only, as on the desktop) |
| System bars, cutouts, keyboard | edge to edge, the page padded clear of them (Android 10 too) |
| Light and dark | `color_scheme = "system"` (the phone's), `"light"` or `"dark"` |
| The page's console | in the device's log (`adb logcat -s aio`) |
| Networking | the app's Rust does it (sync, outbound HTTPS), not the WebView: no mixed-content rule applies to a `ws://` server on the LAN |

### Onto a phone

```sh
ax install notes --android            # the newest APK for the attached phone's CPU, then started
ax install notes --android --build    # build first
ax install notes --android --device R58N… --no-launch
ax install notes --android --emulator # an emulator may be the device
ax install notes --android --apk dist/notes-1.4.7-android-arm64-v8a.apk
```

A phone needs USB debugging on (`adb devices` lists it); `--device`, else
adb's own `ANDROID_SERIAL`, names one of several. The APK is the app's
own exactly (`<id>-<version>-android-<abi>.apk`: `notes` never takes
`notes-pro`'s). An APK older than the sources is refused (it would put the
last build on the phone and say done). `ax release notes --android` publishes the APKs for updates
([release](release.md)).

### Native Android code: `android/`

Files in the app's `android/` folder go over the generated Gradle project,
path for path: a file there replaces the template's whole, or adds one
(a Java class, a resource). Text files get the same placeholders filled
(`{{APPLICATION_ID}}`, `{{NAME}}`, `{{VERSION}}`…). The build says which
files it took, and records what it replaced in `.aio/android-overlay.json`
(commit it): when a newer aio-rs changes a template file you replaced, the
build warns until your copy changes too. An own `MainActivity.java` is
checked for what the template's does (the insets frame and its Android 10
path, the renderer-loss recovery, the splash), each loss named. A
`{{TOKEN}}` no placeholder fills is named too (a typo, or another aio's
key). With `[build.android] strict = true` a drift or a loss fails the
build instead.

## Web

```rust,ignore
// ax create's template (src/lib.rs), compiled by ax's own tests:
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn web() {
    let id = option_env!("AIORT_APP_ID").unwrap_or(env!("CARGO_PKG_NAME"));
    _ = SOURCE.set(aio::Source::Local(aio::Handle::saved(id)));
    aio::web::launch(App);
}
```

- `ax build notes --web` builds the app's library for `wasm32` (release),
  binds it with `wasm-bindgen` (the version the lock names; `ax setup`
  fetches it) and writes `dist/<id>-web/`: the page, a web manifest, the
  Apple Home Screen tags, icons, an offline service worker, and the
  wasm's size raw and gzipped; with `[build.splash]`, iOS's launch images
  (one per iPhone and iPad screen, the splash on its background), with
  `[build.assets]`, those files at their paths. Put it on any HTTPS host, or serve it from
  the app's own server (`aio::http::Http::shell(dir)`).
- `--web --dev`: a debug build whose page shows its errors on it.
- The service worker precaches every file of the build (code checked
  against its digest), answers same-origin requests cache-first, gives
  the page for a route the host has no file for, never caches the app's
  socket or `/__aio/`, and drops older builds' caches. A new build is a
  new `sw.js`: the browser installs it.
- Installing needs a square icon of at least 144 px: a smaller or
  non-square `assets/icon.png` is said, and the generated one used.
- The page's policy allows no inline script and no `eval`; `[app] lang`
  and `dir` are its `<html lang>` and `dir`.
- The cells run in the page: `aio::Handle::saved(id)` is a cell of the
  page, kept in the browser's `localStorage` (per origin: scheme, host and
  port); `aio::web::sync::Remote` is a synced copy of a server's cell over
  its `/ws`, whose offline calls wait in the browser's storage, twin tabs
  mirrored.
- Served by the app's own server, `aio_shell::web::painted` paints each
  viewer's first page on the server (what that viewer may see: no
  `#[server]` field, `visible` rules applied), so it shows before the
  wasm runs; then the page's own views take over.
- Left out in a browser: the store's history, files, blobs, SQL, the
  server, time travel and anything native. Code behind
  `#[cfg(not(target_arch = "wasm32"))]` stays out of the page.
- The browser's address bar and Back button don't follow the app's routes
  ([routing](../ui/routing.md#common-mistakes)).

## Browser UI: `[app] ui = "browser"`

A client-server app may show its UI in the system browser instead of a
window. It serves its page (its web build, painted per viewer) and opens
its address with `?token=<key>` in the default browser, then runs until asked to
stop (`ax stop`). It needs `mode = "client-server"` and a `[server]`;
`ax open notes` opens the page again.

## Service

```rust,ignore
// ax create's template (app/src/main.rs, --template service), compiled by ax's own tests:
let cfg = aio::config!();
let Start::Run(dirs) = aio_shell::start(&cfg, aio::app_version!())? else {
    return Ok(std::process::ExitCode::SUCCESS);
};
let _board = aio::Source::<app::Board>::for_server(&cfg, &dirs)?;
aio_shell::serve(&dirs)?;
Ok(aio::lifecycle::exit_code())
```

- `[app] mode = "server"` and `[server] listen`: the cells are served to
  clients (windows, pages, other apps) until the service is asked to stop;
  it saves first ([sync](../state/sync.md)). `--port`, `--host` and
  `--expose` change where it listens at launch; served beyond loopback it
  turns TLS on ([security](../auth/README.md#tls-and-the-network)).
- Build it as `cli` (the default for a server): a `.tar.gz` whose
  `install.sh` puts it on `PATH` (a `.zip` on Windows), and beside it a
  supervisor file, with the line that installs it (on Linux also a
  system unit, `<bundle>.system.service`, that starts at boot):

| OS | File | Install it |
|---|---|---|
| Linux | `<bundle>.service`, a systemd user unit | copy into `~/.config/systemd/user/`, `systemctl --user enable --now <id>`; `loginctl enable-linger` runs it without a login |
| macOS | `<bundle>.plist`, a launchd agent | copy into `~/Library/LaunchAgents/`, `launchctl bootstrap gui/<uid> …` |
| Windows | `<bundle>.ps1`, a Task Scheduler logon task | `powershell -ExecutionPolicy Bypass -File .\<bundle>.ps1 -Install` (as an administrator) |

- Each starts the installed binary again 5 s after it exits, but not
  after a stop: the supervisor sets `AIO_SUPERVISED=1` and
  `AIO_STOP_STAYS_DOWN=1`, so `ax stop` (or SIGTERM) exits 143 and stays
  down, and an update exits 0 for the supervisor to start the new version.
  All three run as the user who installs them; nothing of the build
  machine is written in them.
- `ax start`, `ax stop`, `ax status`, `ax health`, `ax logs` drive it like
  any app; `ax surface <app> --headless` shows its views without a window.
- The system unit (aio's `build-service`) starts at boot with no login
  and no linger: the binary in `/usr/local/bin/<id>`, run as a user of its
  own that systemd makes (`DynamicUser`), its data in `/var/lib/<id>`
  (`StateDirectory`), `AIO_SUPERVISED`, `AIO_STOP_STAYS_DOWN` and
  `Restart=always` as the user unit. It updates with its package (a
  system install is not the app's to swap). Another user, a container:
  the operator's to write; the binary needs nothing but its flags and
  `AIORT_DATA`.

## CLI

`ax create <name> --template cli` makes a tool whose launcher runs one
command on the tool's saved cell and exits with its code; `aio::cli`
parses arguments and prints answers ([cli apps](../clients/cli-apps.md)).
Build it as `cli`: a `.tar.gz` with `install.sh` (onto `PATH`, no menu
entry), a `.zip` on Windows.

## Thin clients

`[app] mode = "client"` is a window with no cells of its own: it opens a
synced copy of a server's cells (`[client] server = "host:port"`, its host
in `[capabilities] network`, or a server paired with `ax pair`), queues
calls while offline and catches up on reconnect
([sync](../state/sync.md#offline-and-reconnects)). It builds and ships
like any desktop or Android app. A client of a server in another place
keeps the server's address built in; a person can pair it with another.

## How the targets differ

| Concern | Desktop | Android | Web (page cells) | Web (synced) | Service | CLI |
|---|---|---|---|---|---|---|
| where cells run | the app's process | the app's process | the page | the server (copy in the page) | the process | the process, one command |
| the store | SQLite with its log | SQLite with its log | `localStorage` | the server's | SQLite with its log | SQLite with its log |
| durability | `[data] durability` | always full | the browser's | the server's | `[data] durability` | `[data] durability` |
| history, time travel, `ax events` | yes | yes | no | on the server | yes | yes |
| files, blobs, SQL tables | yes | yes | no | through server functions | yes | yes |
| serves clients | `client-server` | `client-server` | no | no | yes | no |
| one copy at a time | yes (a second launch hands over) | yes | per tab | per tab | yes | per command |
| updates | the app's updater ([release](release.md)) | a new APK, offered | a new `sw.js` | a new `sw.js` | the app's updater, installed at once, restarted by its supervisor | the app's updater |
| splash | yes | yes | yes | yes | no | no |

The same `aio.toml` is built into every target; what a target can't use
(a server's `[server]` in a page) is simply not reached.

## Common mistakes

- Native-only code in a view the web build compiles (files, SQL,
  `std::time::Instant`): put it behind `#[cfg(not(target_arch = "wasm32"))]`
  or in a server function ([dev](dev.md#which-code-goes-where)).
- A changed `[build.android] application_id`, `[build.macos] bundle_id`,
  `[app] id` or a lost Android key: the OS treats it as another app and
  won't update the installed one.
- A service built as `desktop`: it gets a window bundle and no
  supervisor; build it as `cli`.
- A macOS bundle or a `.dmg` built on Linux: it needs a Mac
  (`[build.macos] host`).
- Sizing with `cargo build --release`: use `ax build --sizes`.
- Serving the web build over plain http off localhost: the browser
  refuses its service worker (the page says so); use HTTPS.

## Limits

- No iOS target.
- A macOS bundle is built on a Mac (or one over ssh, `[build.macos]
  host`): a macOS binary links Apple's frameworks, whose SDK only a Mac
  may carry; aio's Deno binary cross-compiled to an unsigned `.app` on any host.
- macOS and Windows bundles are not OS-signed or notarized yet.
- Android: no client connect page on the phone yet (pair from the desktop, `ax pair`).
- No Flatpak, Snap, RPM, MSI or Microsoft Store package.
- No one-file "standalone binary with its own browser UI" beyond
  `ui = "browser"` (which needs the app's server).
- No per-target `[app]` overrides: a second app is a second binary with
  its own `aio.toml` (`bin`).

## API

`cargo doc -p aio-rs-shell --no-deps --open`: `start`, `run`, `serve`,
`Window`, `android_app!`, `android::App`, `web::painted`;
`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::web`,
`aio::Source`, `aio::config::Build`, `aio::config::AndroidBuild`,
`aio::config::MacBuild`, `aio::config::Splash`.

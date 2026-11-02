# Real machines

Tests prove the logic and the views. Some questions only a real machine
answers: does the installer open on Windows, does the window look right
at 150% scaling, does the tray icon show on that desktop, does the app
start from Applications on a Mac. aio-rs has three tiers for them:

| Tier | Runs | Who looks | Needs |
|---|---|---|---|
| `ax build <app> --smoke` | the bundle just built, started headless in a throwaway sandbox, stopped, its log checked | nobody: a gate, exit 1 when it fails | this machine (Xvfb, or the runtime's own headless display); a Mac for a macOS bundle; Windows for a Windows one |
| `ax lab <app> <os>` | the app on a real Windows, macOS or Linux desktop, from a folder of its own | **a person**, on that machine's screen | a machine you reach over ssh |
| `ax install <app> --android` | the APK on a phone or an emulator | a person | `adb`, a device or `--emulator` |

`ax lab` is the manual tier: no assertion runs there, nothing in `ax
test` waits for it. It is for the questions people answer by looking.

## `ax build --smoke`: does the bundle start at all

```sh
ax build notes --smoke                           # this OS's bundle, built, then started and stopped
ax build notes --platforms windows,linux --smoke # each Linux bundle here, the Windows one on Windows
```

The bundle is unpacked into a sandbox and started as a person would
start it, on a display nobody sees and with a private session bus, so no
window or dialog reaches anyone's screen. It must:

- open its window in time (a server: boot on a free port; a command-line
  app: answer `--version` with its id and exit 0);
- stop on SIGTERM in time (Windows: when asked with `<app> --quit`),
  leaving no process behind;
- log no panic, problem report, degraded part or page script that
  throws (`ERROR [page] Uncaught …`);
- for a `.deb`: ship what its menu entry names (its `Exec` program, and
  an icon of its `Icon` name under `usr/share/icons`).

`ax smoke <artifact>` smokes a bundle already built, and
`aio::testing::smoke(&bundle)` does it inside a test (aio's `smoke()`):
its report, or a failure with what `ax` said (`ax` from `AIO_AX`, else
`PATH`).

The sandbox, the display and everything started go away again, pass or
fail, with what its start wrote outside the sandbox: its record in the
shared runtime's app list (`apps/*.json`, what `ax prune` reads), a
macOS window bundle no other app names, and Windows' door folder. `[build] smoke = true` in `aio.toml` smokes every build. A Windows
bundle is smoked on Windows: this machine, or the one `AIO_WINDOWS_SSH`
names (`[ssh options] [user@]host`, nothing to install there). A macOS
bundle is smoked on the Mac that built it, where its window shows on the
Mac's screen until it is stopped ([Release](../build/release.md)).

## `ax lab`: the app on a real desktop

```sh
export AIO_LAB_WINDOWS="-i ~/.ssh/lab tester@win-lab"   # [ssh options] [user@]host
ax lab notes windows        # build for Windows, copy it over, start it on that desktop
ax lab notes windows --stop # stop what runs from its folder there, remove the folder
```

| Variable | The machine for |
|---|---|
| `AIO_LAB_WINDOWS` | `ax lab <app> windows`: Windows x64, its PowerShell over ssh |
| `AIO_LAB_MACOS` | `ax lab <app> macos`: an Apple-silicon Mac |
| `AIO_LAB_LINUX` | `ax lab <app> linux`: a Linux of this machine's CPU, with a desktop |

Each is `[ssh options] [user@]host`, as for `ssh`: `-p 2222 -i key
tester@lab` (`-p` becomes `-P` for the copy). The machine must accept
the key without a prompt (`BatchMode`).

With no machine named (or `--boot`), `ax lab` boots aio's lab here in
Docker instead, the same images and container names as `am lab`: a
Windows or macOS VM (KVM), an XFCE desktop, or the Android emulator, its
viewer on a loopback port and the app's `dist/` in it at `/shared`
([ax lab](../clients/ax.md#ax-lab)):

```sh
ax build notes --android && ax lab notes android   # boots, waits, installs the APK
ax lab notes linux                                  # an XFCE desktop, up in seconds
ax lab notes windows --status
ax lab notes windows --reset                        # stopped, its VM disk deleted
```

What `ax lab <app> <os>` does on a machine over ssh:

1. builds the app for that OS (as `ax build <app> --platforms <os>`; a
   Windows build is cross-built here, a macOS one needs a Mac: the app's
   `[build.macos] host` or `AIO_MACOS_SSH`);
2. makes `aio-rs-test/lab/<id>` under that machine's home, and refuses
   when it is there already (an earlier lab not stopped);
3. copies the bundle there with `scp` and unpacks it;
4. starts it on the desktop of the person signed in there, with its
   homes inside that folder, so its data, runtime and caches go there and
   nowhere else:

| OS | Started by | Its homes |
|---|---|---|
| Windows | a one-shot scheduled task, the way into the signed-in desktop (none signed in: it fails, saying so); the task is deleted once it ran | `LOCALAPPDATA` and `APPDATA` in the lab folder |
| macOS | `open -n` of the `.app` (a bare binary: run in the background) | `HOME` in the lab folder |
| Linux | in the background on its `DISPLAY` (`:0` when unset) | `HOME` in the lab folder, the XDG variables cleared |

It prints where it started the app, then the line that ends it:
`stop it, and remove all of it there: ax lab notes windows --stop`.

`--stop` stops only the processes whose executable lies in that app's
lab folder, by their path, never by a name, then removes the folder and
the empty `aio-rs-test/lab` folders above it. Nothing else on the
machine is touched, so a lab machine can be a person's own test box.

### What to look at there

- The window: its chrome, its size and position on first start, high-DPI
  scaling, dark mode, the fonts that machine has.
- The OS's own surfaces: SmartScreen or Gatekeeper, the tray or menu bar
  icon, notifications, file pickers, the taskbar or Dock entry.
- A second start: the app finds its data, one instance runs.
- Updates and uninstall, from the installed app (`ax install` on that
  OS).

`ax shot` and `ax surface` work on a lab app as on any running app when
you run them on that machine; from here, look at the machine's screen.

## Phones

`ax install <app> --android` puts the newest APK for the attached
phone's CPU on it and starts it; it refuses an APK older than the
sources (`--build` builds first). `--emulator` lets an emulator be the
device, `--device serial` picks one of several, `--apk file` installs a
given APK, `--no-launch` only installs. Build the APKs with `ax build
<app> --android` ([Targets](../build/targets.md)).

## Choosing a tier

| Question | Tier |
|---|---|
| Does a method, a refusal, a schedule work? | [cell tests](cells.md) |
| Does the view show and do the right thing? | [view tests](ui.md) |
| Does the built bundle start and stop cleanly on this OS? | `ax build --smoke` |
| Does the Windows bundle start on Windows? | `ax build --platforms windows --smoke` with `AIO_WINDOWS_SSH` |
| Does it look right, does the OS let it in, does the tray work? | `ax lab <app> <os>` |
| Does the APK install and draw its first screen? | `ax install <app> --android` |

## What aio has that aio-rs does without

| aio | aio-rs |
|---|---|
| `--dist`, `--tunnel` (a viewer through `docker exec` when docker's port publishing is broken) | none: the app's `dist/` is shared; the viewer is a published loopback port |
| `deno task lab`: install, create, dev and build scenarios in fresh Ubuntu containers, with a real-browser check of the screen | `ax lab <app> linux --boot` (aio's lab, in Docker) for a person; the framework's own gates are GitHub CI on Linux, Windows and macOS and `scripts/mutants` (a built `.deb` broken on purpose must fail its smoke test) |
| `deno task test:wine`: the Windows `.exe` under Wine | none: Windows bundles are smoked on Windows (`AIO_WINDOWS_SSH`, `./ci-windows.sh`) |

## Common mistakes

- Starting a second lab of the same app before `--stop`: refused, the
  folder is there already. Stop the first.
- `ax lab` on Windows with nobody signed in on the desktop: the
  scheduled task has no desktop to start on, and the lab says so.
- A lab machine whose ssh key needs a passphrase prompt: `BatchMode`
  refuses it. Load the key into an agent first.
- Treating a lab as a test: nothing there fails a build. Put what can be
  checked in a test or in `--smoke`, and keep the lab for what needs eyes.

## API

`ax lab --help`, `ax build --help`, `ax install --help`;
[Release](../build/release.md), [Targets](../build/targets.md).

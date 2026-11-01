# ax command reference

`ax` makes, runs, inspects and ships aio-rs apps, for people and for coding
agents alike: no `ps`, no `kill`, no `curl`. This page is every verb and
flag, grouped by what you are doing. It is checked against the verb table
`ax help` prints (`scripts/docs-check`), so a verb or flag shown here
exists. `ax help <verb>` (or `ax <verb> --help`) shows one verb in full.

```sh
ax help                  # every verb, one line each
ax help state            # one verb: its usage, what it does, its flags
ax state --help          # the same
```

Prefer a window? `ax ui` is the visual app manager: the same verbs behind
buttons ([below](#ax-ui-the-visual-app-manager)).

## Which verb answers which question

| Question | Verb |
|---|---|
| Does this machine have what making apps needs? | `ax setup` |
| Run it while I edit | `ax dev` |
| Run it in the background, and is it up? | `ax start`, `ax status`, `ax health` |
| What is in a cell right now? | `ax state`, `ax snapshot` |
| What happened, and in what order? | `ax events`, `ax logs`, `ax errors` |
| Call a method, check the result | `ax call`, `ax expect` |
| Step back through a cell's history | `ax timetravel` |
| Reproduce a bug, turn it into a test | `ax replay`, `ax record` |
| What does the window show? Click it | `ax surface`, `ax trigger`, `ax shot`, `ax eval` |
| One component alone, with props I pick | `ax preview` |
| Is the running process the code on disk? | `ax doctor` |
| Where does it keep its data? | `ax data` |
| What does it cost (memory, bytes sent)? | `ax heap`, `ax metrics`, `ax cost` |
| Make the files a person installs | `ax build`, `ax release` |
| Find what types can't check | `ax lint` |

## Conventions

### Naming the app

Every verb about an app takes `<app>` first: the app's package name in its
workspace, or its id for an installed app.

- **Inside an app's folder** `<app>` may be left out: `ax` asks Cargo which
  package the folder belongs to. Outside any workspace it says `no app here
  (no Cargo workspace): give one, or --app <name>`.
- **Beside an app's folder** (in no workspace, `./notes/` holding a
  `Cargo.toml`), the verbs about an app's source (`ax dev`, `ax test`,
  `ax build`, `ax release`, `ax keygen`, `ax preview`, `ax config`, `ax lab`,
  `ax install`) step into it as after `cd notes`, and say so on stderr.
- **`--app <name>`** names it anywhere, for any verb that takes `<app>`.
- **In a workspace**, `<app>` is found by its `aio.toml`: its id (its data's
  home) comes from `[app] id`, not from the package name. Outside one, the
  word is the id of an installed app.

### Which copy of the app

One app may run as several copies, each with its own data
([files](../persistence/files.md)). These pick one, as the app's own launch
takes them:

| Flag | Picks |
|---|---|
| `<app>@<profile>` | profile `<profile>` (a home beside the app's own) |
| `--profile <name>` | the same, as a flag |
| `--instance <name>` | a private instance, under the tool home's `instances/<name>/` |
| `--home <dir>` | the copy whose home is that folder |

- Asked for none, and the app isn't running from its own home but from
  exactly one other (a profile, an instance), the verbs follow it and say
  `ax: notes runs from --profile qa, not its own home: following it`. Two
  others running: refused, naming both, so you pick.
- `notes@qa` and `--profile other` together are refused: one names the
  profile.
- Verbs that work on the app's source, not a running copy (`ax dev`,
  `ax keygen`, `ax release`, `ax update`, `ax build`, `ax config`,
  `ax remove`, `ax install`, `ax theme adopt`) refuse `--profile`,
  `--instance` and `--home` (`… is about no running app: --profile does
  nothing for it`). `ax dev` takes `--instance` among the app's own args.
- **`AIORT_DATA=<dir>`** points at one app's folders instead of the tool
  home: throwaway runs and tests, nothing touched elsewhere. Both the app
  and `ax` read it, so set it for both.

### Global flags

Every verb takes these:

| Flag | Does |
|---|---|
| `--json` | one JSON object on stdout, for scripts (below) |
| `--quiet` | nothing on stdout when it worked: the exit code says it; a failure still says why on stderr |
| `--app <name>` | the app, wherever you are |
| `--profile <name>`, `--instance <name>`, `--home <dir>` | which copy |
| `--help` (`-h`) | this verb's help, and nothing run |
| `--version` (`-V`) | `aio-rs <version>`, and nothing run |

`--json` wins over `--quiet`.

### How the command line is read

One gate reads every command line before anything runs. What it can't read
is refused with exit 2 and the right usage, never guessed or dropped:

- An unknown flag, with the nearest one: `events: unknown flag --lats (did
  you mean --last?)`, then the flags the verb takes. An unknown verb gets
  the nearest verb.
- A flag that needs a value and has none (`--last needs a value`), a value
  on a switch (`--json takes no value`), a word too many (`health: one word
  too many: extra`) or too few.
- `--flag value` and `--flag=value` are the same.
- `--` ends the flags: every word after it is a word, however it looks
  (`ax call notes notes add -- --draft` passes the text `--draft`).
- Flags are `--words`. The one-letter ones are `-f` (`ax logs --follow`),
  `-y` (`ax prune --yes`), `-h` and `-V`; any other `-x` is refused (a
  negative number such as `-3` is a word).
- Verbs that moved say where to: aio's `dispatch` names `ax call`, a bare
  `theme` names `ax theme adopt`.

### Output and exit codes

- Text in a terminal, the same text in a pipe; `--json` for a script.
- With `--json`, every verb prints one object: `{"ok":true,"value":…}` when
  the answer is JSON (a state, a call's answer, a list), `{"ok":true,
  "text":"…"}` when it is text, `{"ok":false,"error":"…","exit":1}` when it
  failed. The verbs scripts branch on answer objects, not sentences:
  `start`/`restart` (`app`, `state`, `pid`, `version`, `startedAt`, `cdp`,
  `log`), `stop`/`kill` (`how`: stopped, sigterm, sigkill or asked;
  `seconds`, `discarded`), `errors` (`build`, `starts` with their `lines`,
  `total`), `health` (`healthy`, the `health.json`), `expect` (`pass`, the
  check), `doctor` (`findings`, `said`), `backup` (`file`, `bytes`),
  `shot` (`file`, `bytes`, `width`, `height`), `build` (the manifest) and
  `create`. A failure that has its own object (an unhealthy `health`, a
  failed `expect`, one app of several) carries it as `value`.
- Exit codes: **0** ok, **1** failed, **2** bad usage. Two verbs have their
  own: `ax status` (0 started, 1 stopped, 2 on the way) and `ax lint` (0 no
  errors, 1 errors, 2 bad usage).
- Errors go to stderr, prefixed `ax:`; notes about what `ax` decided for you
  (following another home, stepping into a folder) too.

### The live app, or what it stored

Verbs that read state ask the **running** app over its local door and fall
back to its store when it isn't running.

- **The local door** is a private socket each running app opens (a Unix
  socket in a private folder; a named pipe on Windows): no network, no key,
  only this OS user can open it. It is how `ax`, `ax ui` and other apps of
  this user reach a running cell.
- **A debug build's door has the operator's rights**: any method, server
  fields, time travel, a loaded state. **A release build's door** answers
  as a client with the app's key would: the `#[client]` methods as their
  rules say, what `visible` shows. So `ax state` on an installed app shows
  what a client would see, and `ax call` can only call what a client could
  ([visibility](../state/sync.md#who-may-call-what)).
- **When it isn't running**, `ax state` reads the newest stored snapshot,
  read-only (SQLite lets a reader in beside a writer), and says so,
  with how many newer events the snapshot doesn't hold yet:

  ```text
  todo v3 after event 41 (stored: the app isn't running)
  ```
- **Nothing is queued**: a call to an app that isn't running says `notes
  isn't running (nothing was sent)`.
- **`--as user`** reads as that user (default: the OS user running `ax`),
  where the cell shows each user their own view.

### The build folder

Every `cargo` and `dx` that `ax` runs builds into one folder per machine:
`CARGO_TARGET_DIR` when it is set, else `target/` in the tool home. An
app's libraries are built once for every app, and no build output lands
in a repo.

## Make and run

### `ax setup`

```text
ax setup [--check]
```

What making apps needs. Checks Rust and the C toolchain it links with;
fetches prebuilt, SHA-256 checked, the shared Electron runtime (then
starts it once, so a missing system library is named), `dx` (the Dioxus
CLI `ax dev` hot-patches with) and the wasm-bindgen the web build binds
with; checks the Rust targets Android and the web build need; lists the
optional tools. Each missing one is said with the command that gets it.

| Flag | Does |
|---|---|
| `--check` | look only, fetch nothing |

Exit 1 while a needed tool is missing. Run it once per machine, and again
after upgrading aio-rs. [Setup](../basics/setup.md).

### `ax create`

```text
ax create <name> [--template t] [--id id] [--aio-version v] [--aio dir] [--target a,b] [--css tailwind] [--force]
```

A new app in `./<name>` that runs, tests and builds as it is, on any
machine after a clone: its own workspace and git repository (a first
commit; not inside another repository), `aio.toml`, a test, a `README.md`,
and for coding agents `AGENTS.md` (the rules and the loop inline, the rest
by `ax agent` and the guide's URL) and a `CLAUDE.md` importing it. aio-rs
comes from git at a release tag; nothing of the machine it was made on is
written in it.

| Flag | Does |
|---|---|
| `--template app` | a window app (the default) |
| `--template cli` | a command-line tool ([cli apps](cli-apps.md)) |
| `--template service` | a served app, no window |
| `--id id` | its id, its data's home (default: the name; refused when an installed app has it) |
| `--aio-version v` | the aio-rs release it takes from git: `0.2.0`, `main` (the branch), `latest` (the newest release); default: this `ax`'s |
| `--aio dir` | an aio-rs checkout instead, named relative to the app (they share a folder below the root) |
| `--target a,b` | its `[build] targets` (`ax build <app> --list` names the kinds) |
| `--css tailwind` | `[build] css` runs Tailwind on `src/app.css` into `assets/style.css` (a window app) |
| `--force` | write the template over a folder that holds files |
| `--json` | the folder, its files, its aio-rs and the next steps, as one object |

A failure part-way leaves nothing behind; with `--force`, the files it
wrote over are put back.

```sh
ax create notes
cd notes
ax test notes && ax dev notes
```

[Quickstart](../basics/quickstart.md), [the files it makes](../basics/project.md).

### `ax pin`

```text
ax pin <app> [version] [--major]
```

Which aio-rs the app uses, and how to change it (aio's `am pin`). `<app>`
is its name or folder, left out inside it. No running app is needed.

With no version it changes nothing and says what each `Cargo.toml` file
asks for (a release tag, a branch, a checkout), what `Cargo.lock` built
(version, source, commit), this `ax`'s version and whether they match,
and aio-rs's releases (its git tags; offline: said). Then, as aio does:
drift (Cargo.lock built something Cargo.toml didn't ask for: exit 1),
files that disagree, no Cargo.lock yet, a local-dev pin, how many releases
behind the newest it is, and what to type next.

With a version, every aio-rs dependency of every `Cargo.toml` (workspace
dependencies too) moves there, and `Cargo.lock` follows (`cargo update -p`
for the aio-rs crates only; offline, from what cargo has, said); a move
cargo can't resolve changes nothing:

| version | moves to |
|---|---|
| `0.2.0`, `v0.2.0`, `0.2` | that release's git tag (refused when aio-rs has no such tag) |
| `latest` | the newest release in the app's major; `--major`: any (a breaking move, said) |
| `main` | the branch tip; `Cargo.lock` holds the exact commit; re-run to advance |
| `../aio-rs` | an aio-rs checkout, named relative to the app: this machine only (said) |

A move back to an older release says what may break. `--json`: the facts,
or what moved, as one object.

```text
$ ax pin notes
  aio-rs    v0.1.0  Cargo.toml, app/Cargo.toml
  built     0.1.0 (tag v0.1.0 @ ab0cada)
  ax        0.1.0  matches
  latest    v0.1.0
  releases  v0.1.0

  ax pin latest     the newest release in this app's major
  ax pin <version>  switch this app (Cargo.toml and Cargo.lock)
  ax pin main       follow the branch tip (a moving target)
  ax pin <path>     follow an aio-rs checkout on this machine
$ ax pin 0.1
pinned to aio-rs v0.1.0  (resolved from "0.1")
  recorded in Cargo.toml, app/Cargo.toml
  Cargo.lock: aio-rs 0.1.0 @ ab0cada
  commit Cargo.toml and Cargo.lock so a clone builds against the same version
```

### `ax dev`

```text
ax dev <app> [args…]
```

Runs the app in your terminal while you edit. With dx (`ax setup` fetches
it) an edit to a function hot-patches into the running app, its state
kept; an edit it can't patch (a new type, a changed struct) rebuilds and
restarts. Without dx every edit rebuilds and restarts. A failed build
leaves the last good one running; `ax errors` shows why.

The words after the app go to the app, except these, which `ax dev` reads:

| Arg | Does |
|---|---|
| `--instance <n>` | a private copy with its own data |
| `--fresh` | that instance on new data (needs `--instance`: only a private instance's data is thrown away) |
| `--display auto` | with a terminal, your desktop; with none (an agent), a nested display, and no browser tab opened (the default) |
| `--display isolated`, `current`, `:N` | always nested; always your desktop; that X display |
| `--size 390x844` | the window at that size for this run (its saved size untouched) |
| `--watch=false` | build and run it once, no watching |
| `--watch=src/ui,assets` | rebuild on changes in those folders of the app only |
| `--cdp[=port]` | the window opens the DevTools protocol on loopback (a free port without one), for other tools |
| `--tui` | dx's own screen and keys instead of `ax`'s compact lines |
| `--android [--device serial] [--native]` | on the attached emulator or phone (`ANDROID_SERIAL`; none: `AIO_AVD` booted): its web build in a dev APK installed once beside the app, each edit reloading it (`--native`, or no web build: the app itself for that CPU, again on every save); its log in the terminal |
| `--web` | its web build (debug) served on loopback, its tabs live: a saved stylesheet shows at once, another edit builds anew and reloads |

```sh
ax dev notes                              # the window, hot-patched
ax dev notes --instance a1 --fresh        # a throwaway copy on new data
ax dev notes --size 390x844               # a phone-sized window
ax dev notes --web                        # in the browser instead
ax dev notes -- --verbose                 # an arg for the app itself
```

dx's whole output is in the app's `logs/dev.log`. `ax shot`, `ax eval`,
`ax surface` and `ax trigger` reach a dev build without `--cdp`.
[Develop](../build/README.md#develop-ax-dev).

### `ax test`

```text
ax test <app> [filter] [--video out] [--pace ms] [--android] [--device serial]
```

The app's tests (`cargo test`), UI tests included, headless: no window on
the screen.

| Arg, flag | Does |
|---|---|
| `filter` | only tests whose name holds it |
| `--video out` | record each UI test, no test changed: `out` a folder (a video per test) or one `.mp4`/`.webm` |
| `--pace ms` | each step this long on screen in the video (800) |
| `--android` | the emulator lane instead: its APK for the device built, installed and started; passed once its window is ready and it still runs (`AIO_AVD` booted when none is attached) |
| `--device serial` | with `--android`: that device (else `ANDROID_SERIAL`) |

```sh
ax test notes
ax test notes add_rejects_empty
ax test notes --video videos/
```

[Testing](../testing/README.md).

### `ax lint`

```text
ax lint [dir] [--json] [--no-hints|--quiet-hints] [--no-clippy] [--safe-fix] [--fix] [--from version] [--tests]
```

What the compiler and the macros can't check: credentials in source,
assets and cell fields every client gets; impure methods (I/O, the clock,
randomness, threads); hooks off a component's top level; markup without an
accessible name; duplicate cell ids; cells no test names; `aio.toml`;
`.env` files git would commit. Then clippy, with aio's lints.

| Arg, flag | Does |
|---|---|
| `dir` | the project: the nearest `Cargo.toml` at or above it (default: here) |
| `--json` | one JSON document (hints always in) |
| `--no-hints`, `--quiet-hints` | hint lines left out (still counted) |
| `--no-clippy` | aio's own checks only: fast, no build |
| `--safe-fix` | clippy's machine-applicable fixes first, then lint what is left |
| `--fix` | move an app of the older layout to today's (what `ax create` makes) first, then lint |
| `--from version` | only what aio-rs changed after that version (0.1.0) that the app still uses (aio's `am migrate`), as errors: CI fails on them; no clippy |
| `--tests` | only the test shapes a newer aio-rs turns red (TestUi's), in order, as warnings (a prediction: exit 0) |

Exit 0 with no errors (warnings and hints don't fail), 1 with errors, 2 on
bad usage. Silence one finding, with its reason, on its line or in the
comment block above: `// aio-ok: <why>` or `// aio-ok(<area>): <why>`. A
silence without a reason is itself a finding. [Lint](../testing/README.md).

### `ax start`

```text
ax start <app> [arg…] [--wait s] [--no-wait] [--fresh] [--display d] [--cdp] [--all]
```

Runs the app in the background, detached from your terminal (its own
process group: a Ctrl-C there doesn't reach it), and waits until it is
up: its lock held, its `health.json` written by this start, and, for a
window app, its window ready. In a workspace it is built first; elsewhere
the installed one runs.

| Flag | Does |
|---|---|
| `--wait s` | wait at most `s` seconds for it to be up (30) |
| `--no-wait` | return once it is spawned (`--wait 0` too); `ax status` says when it is up |
| `--fresh`, `--display d`, `--cdp` | as `ax dev`; an `--instance` started `--fresh` throws its data away again when `ax stop` ends it |
| `--all` | every app of this workspace (each binary beside an `aio.toml`) |

Args after `--` go to the app. Its output goes to `logs/start.log`; a start
that ends at once says why with that log's tail. The launch is recorded in
`logs/start.json` (args, binary, pid, who), so `ax restart` replays it and
`ax apps` says who started it.

```sh
ax start notes && ax status notes
ax start notes --wait 90 -- --seed demo
ax start --all
```

One instance per home: a second launch of a running app hands its
arguments to the running one, which comes to the front, and exits.

### `ax stop`

```text
ax stop <app> [app…] [--all] [--wait s] [--no-wait]
```

Asks the app to close, on any OS, over its local door, and waits until it
is gone (its lock free: the OS drops it with the process, so "gone" is
never a guess). It runs its own shutdown: stops, the last write, the
window closed.

| Flag | Does |
|---|---|
| `--wait s` | how long asking may take (15); then SIGTERM, then SIGKILL, 5 s each |
| `--no-wait` | ask it to close and return: no waiting, no signal |
| `--all` | every running app of this workspace; the rest are said |

A private instance started with `--fresh` (`ax start notes --instance a1
--fresh`) is a throwaway run: once `ax stop` has seen it gone, its data is
thrown away too (`its --fresh data thrown away`, `"discarded": true`).

It says how it ended: `notes: stopped (0.4 s)`, `stopped by SIGTERM: asking
didn't do it within 15 s`, or `killed: neither asking nor SIGTERM stopped
it`. An app that isn't running is an error (exit 1); in a script, `ax stop
notes || true` when that's fine.

Never end an app by process name (`pkill`): a name matches every copy and
every app built alike. The pid `ax` signals comes from the app's own
`health.json`, never from a process list.

### `ax restart`

```text
ax restart <app> [arg…] [--wait s] [--cdp] [--all]
```

Stops it if it runs, then starts it again with the flags it was started
with. A flag given here replaces the recorded one of its name; args after
`--` replace the recorded args whole.

### `ax kill`

```text
ax kill <app>
```

Ends the app now, no asking: no shutdown runs. The pid comes from its
`health.json`. For an app that won't answer `ax stop`.

### `ax status`

```text
ax status <app> [app…]
```

One line an app, from its lock and `health.json`:

| Status | Exit | Means |
|---|---|---|
| `started` | 0 | up and healthy (`started but … stuck`: exit 1) |
| `starting` | 2 | locked, no health yet |
| `stopping` | 2 | on its way down |
| `maintenance` | 2 | `ax restore` holds it; wait and ask again |
| `stopped` | 1 | not running; `crashed or killed, last seen …` when it didn't stop cleanly |

For several apps: 0 when all are started, else 1 when any is stopped,
else 2.

```sh
until ax status notes; do test $? -eq 2 || exit 1; sleep 1; done   # wait out a start
```

### `ax apps`

```text
ax apps
```

The aio-rs apps and private instances on this machine: each one's id,
running or not, its pid, version and port when it runs, who started it
(`ax start`, or `other`: `ax dev`, a launcher, a terminal), and the aio-rs
it runs on when that isn't this `ax`'s (two checkouts on one machine is the
usual dev setup, and a mismatch the first thing to rule out).

### `ax open`

```text
ax open <app> [--print]
```

The running app's page in the browser, once it answers. `--print`: only
its address, for a script.

### `ax config`

```text
ax config <app>
```

The app's `aio.toml` as its next build bakes it in: checked, every default
filled in, with the file it came from. Run it in the app's workspace (an
installed app's config is built into it).
[Every key](../reference/aio-toml.md).

### `ax data`

```text
ax data <app>
```

Where the app keeps everything (data, logs, cache, keys, files), each
folder's size, and what to back up. [Files](../persistence/files.md).

### `ax doctor`

```text
ax doctor <app> [--harden]
```

Is the running app the code on disk? A process keeps the code it started
with: a rebuild or an edit since is a finding, with its fix (stop it and start it again). It also says when the app runs
another aio-rs than this `ax`. Then who decided each setting it runs with: `aio.toml`, the
folders, the environment variables aio reads (a token's value is never
shown).

Hints, running or not, each saying what ends it: a window app whose binary
pins an Electron older than the one this aio-rs is tested with (build it
with this aio-rs), an app served `[server] open`, and an opt-in security
switch it hasn't decided on: a server beyond loopback that takes a socket
saying no Origin. `--harden` writes those switches into its `aio.toml`
(`[server] strict_origin = true`) and says each; a value the app already
set, either way, stays (aio's `am fix --harden`).

### `ax theme adopt`

```text
ax theme adopt <app> [--force]
```

aio-rs's theme (the tokens for the app's `[ui]` hue and the whole look)
into the app as `assets/aio-theme.css`, its own file from then on: in its
git history, edited like any stylesheet; no aio-rs upgrade moves its look.
`--force`: write over the one there. [Theming](../ui/theming.md).

### Common mistakes

- Running a verb in the wrong folder: inside another app's workspace the
  verbs find that app. Name it (`--app notes`) when unsure.
- `ax start` then reading state at once: `ax start` returns when the app is
  up, so this works; a script that started it some other way waits with
  `ax status` (exit 2 means "on the way").
- Expecting `ax dev` and `ax start` to share a copy: they do (one home), so
  a second launch hands over to the first. Use `--instance` for a second
  copy.
- `--fresh` without `--instance`: refused, so a person's own data is never
  thrown away.

## `ax ui`: the visual app manager

```text
ax ui [--window] [--browser]
```

A window of aio-rs's own, on the shared runtime, for the apps on this
machine: aio's manager (amui), ported screen for screen, dark or light
with the OS. What it does to an app is an `ax` verb run as a child (a view
of the verbs, never a second way in); what it reads of a project is read
only, and never a secret folder (`.ssh`, `.gnupg`, `.aws`).

| Flag | Does |
|---|---|
| (none) | opens detached; your terminal is free |
| `--window` | opens here, until it closes |
| `--browser` | serves the same view to this machine's browser on a loopback port, behind a key made for the run (its link sets the key as a cookie), until Ctrl-C |

- **On the left**: the running apps and private instances (`ax apps`),
  or all of them (remembered), searched by name or folder; Rescan; a new
  app by name (`ax create` in `~/aio-apps`, its name made safe). Stopped
  projects are found too, three levels deep: in `~/aio-apps`, your home
  folder (never its app state or machine folders), the current folder,
  the running apps' folders, and the folders in `AIORT_UI_ROOTS`
  (`:`-separated, `;` on Windows).
- **On the right**, the chosen app: its status, build (dev or release),
  folder and when it was read; Refresh; Start; Restart or Stop, asked
  first in a bar under the name. `ax ui` lists itself and never
  offers to stop itself. A running app is read again every 2.5 s, the list
  every 9 s.

| Tab | Shows | From |
|---|---|---|
| Overview | status, build, port, pid, uptime, clients, CPU, memory, state; process, cell health, project, config, build targets, newest errors, schedules | `health.json`, `ax metrics`, `ax errors`, `ax schedules`, `/proc` |
| Cells | each cell's methods: run one with a JSON list of arguments, confirmed naming the exact call (and saying when it gets none); a link to the cell's source | `ax peer`, `ax call` |
| State | each cell's fields as a foldable tree, each marked saved (`disk`) and sent to the window (`ui`), with its JSON size | `ax snapshot`, `ax peer` |
| Metrics | trends of CPU, memory, state, the slowest call's p95 and pushes (the last 60 readings); calls, memory, linked clients, cell sizes, recent actions, the window's components | `ax cost`, `ax metrics`, `ax clients`, `ax events`, `ax components` |
| Logs | the newest 500 lines of all, the framework's, the warnings or the page's, by level and word; Follow reads again every 2 s | the app's `logs/` |
| Tasks | `ax dev`, `test`, `lint`, `build` (each target), `doctor`, `config`, `data` in the project, its output live; Stop task ends it (and its children), and 5 minutes do | `ax` |
| Codebase | the app's folder, or its repository, as a foldable, filtered tree (the first 1200; no `target`, `.git`, `node_modules`, `dist`); a file highlighted, up to 2 MB | read only |

A verb that fails says why where its answer would be, and an answer is
never shown as current once a read of it failed.

Where amui reads what aio-rs hasn't, it shows aio-rs's counterpart:
the cells' state for the V8 heap, the slowest call's p95 for reduce p95,
pushes for the queue depth, degraded cells for breakers. CPU every 2.5 s
needs `/proc` (Linux); elsewhere it is the app's own, every 10 s.

## Inspect a running app

### `ax state`

```text
ax state <app> <cell> [path] [--watch] [--as user]
```

A cell's state, live from the running app, else its newest stored snapshot
([above](#the-live-app-or-what-it-stored)). With `path`, only that value,
printed alone so a script reads it as it is.

| Path | Picks |
|---|---|
| `items.0.title` | keys and indexes |
| `notes.*.text` | `*`: every item |
| `notes[?done]` | items whose `done` is truthy (not null, false, 0 or "") |
| `notes[?done=true].text` | items whose `done` equals the value (JSON when it reads as JSON, else text), then a field |
| `notes[?tag!=work]` | items whose field differs |
| `owner.{id,name}` | some fields of an object |

| Flag | Does |
|---|---|
| `--watch` | a line now and after each change, until the app closes the link |
| `--as user` | as that user sees it (default: you) |

```sh
ax state notes todo
ax state notes todo 'items[?done=false].title'
ax state notes todo items.0 --json
ax state notes todo count --watch
```

Quote a path with `?`, `*`, `{` or `[` in it: the shell reads them. A path
that misses says how far it got and prints the syntax.

### `ax snapshot`, `ax snapshot save`, `ax snapshot load`

```text
ax snapshot <app> [--as user]
ax snapshot save <app> [file] [--as user]
ax snapshot load <app> <file> [--force]
```

- `ax snapshot`: every cell's state as one JSON object (live per cell, else
  stored; `null` for a cell with no snapshot yet).
- `ax snapshot save`: that object as a new file (default
  `<id>-snapshot.json`; an existing file is refused, never overwritten).
- `ax snapshot load`: each cell the file holds put in place on the running
  app, each as a new event, and it says how to step back (`ax timetravel
  notes todo goto 41`). A file of another cell set than the running app's
  (another app's snapshot, an older build's) is refused, naming the cells
  that differ; `--force` loads the cells the app has and says the rest. A
  snapshot taken `--as` someone holds only what they saw, so loading it
  puts back only that.

```sh
ax snapshot save notes before.json
ax call notes todo clear
ax snapshot load notes before.json
```

A snapshot is for looking and for moving state between copies; a backup
(`ax backup`) is for keeping data.

### `ax events`

```text
ax events <app> [cell] [--last N] [--diff] [--from file]
```

The newest events of the app's log, oldest first: their number (seq), time,
cell, method, arguments and caller. A `#[redact]` call's arguments show as
`[redacted]`.

| Flag | Does |
|---|---|
| `--last N` | how many (20) |
| `--diff` | each with the changes it made, `path: before → after` |
| `--from file` | another store, or a backup file |

```sh
ax events notes todo --last 5 --diff
```

### `ax timeline`

```text
ax timeline <app> [cell] [--last N] [--from file]
```

What happened and what it changed (aio's `am timeline`): the newest events
of every cell (or one), oldest first, each with its state diff. The states
come from the running app; a stopped app's store, or another one
(`--from`: a backup, a copy), is replayed by the app's own code, built by
cargo as its tests are (in its workspace), on a copy: nothing in it moves.
`ax events --diff` is the same.

### `ax timetravel`

```text
ax timetravel <app> <cell> [step] [seq]
```

Where the running cell is in its history, its newest calls and their time.
`step` moves it:

| Step | Does |
|---|---|
| `undo`, `redo` | one call back, forward |
| `goto <seq>` | to just after event `seq` (the numbers `ax events` lists) |
| `pause`, `resume` | hold it where it is, let it go again |

Each move pauses the cell (calls refused, schedules held) until `resume`,
and each is a new event in the log: nothing is lost, and every move can be
stepped back from. A debug build only.
[Time travel](../debugging/README.md#4-time-travel).

### `ax migrations`

```text
ax migrations <app>
```

Each stored cell's version against the running app's, and its shape drift:
fields stored but no longer declared (this build doesn't read them) or
stored with another type (they don't load). `null` and `[]` fit anything.
[Migrations](../persistence/README.md).

### `ax schedules`

```text
ax schedules <app>
```

Saved schedules (`#[every]`, `#[cron]`, `cx.every`), soonest first: when,
cell, key, method, interval or cron line.

### `ax sql` and `ax tables`

```text
ax sql <app> [query…] [--tables]
ax tables <app>
```

One read-only query of the store, answered as JSON rows. The store is opened
read-only and the statement must be one that writes nothing (SQLite judges
it, not a scan of the text). An answer over 10 000 rows is refused, never
cut. `--tables` and `ax tables`: the store's tables.

```sh
ax sql notes "SELECT cell, COUNT(*) FROM log GROUP BY cell"
```

[The store's tables](../persistence/sql.md).

### `ax logs`

```text
ax logs <app> [word] [--last N] [--level l] [--tag t] [--since s] [--client] [--follow]
```

The newest log events, oldest first. The unit is the event, not the line:
an error keeps its stack.

| Arg, flag | Does |
|---|---|
| `word` | only events holding it |
| `--last N` | how many (50) |
| `--level l` | at level `l` or worse (`debug` < `info` < `warn` < `error`) |
| `--tag t` | of category `t`, and `t:*` (`cell` finds `cell:todo`) |
| `--since s` | newer than `15m`, `2h`, `1d` or a time (`2026-10-28T10:00`) |
| `--client` | the window's lines (its console) |
| `--follow`, `-f` | then each new one, until Ctrl-C |

```sh
ax logs notes --level warn --since 15m
ax logs notes --tag cell:todo -f
```

### `ax errors`

```text
ax errors <app> [--last N] [--since s]
```

`ax dev`'s failed build first (the app runs its last good build meanwhile),
then the newest warnings, errors and panics (20), each under the start of
the app it came from. The first thing to run when something broke.
`--since 15m` (or `2h`, `2026-10-28T10:00`): only newer ones; `--since
reload`: only since the app's newest start, or `ax dev`'s hot patch after
it. aio's `--lines N` is `--last N`.

### `ax report`

```text
ax report <app> [id]
```

The app's problem reports, newest first; one in full by id (its first
letters are enough). [Problem reports](../debugging/README.md#5-problem-reports-and-health).

### `ax health`

```text
ax health <app>
```

Healthy, or why not: exit 1 when it is degraded, stuck or not running.
Read from the `health.json` the app writes, so it answers without reaching
the app.

### `ax metrics`

```text
ax metrics <app> [--as user]
```

Uptime, open links, schedules and each cell's size: live when the app runs
(a cell without a door: its stored size), else stored.

### `ax heap`, `ax top`

```text
ax heap <app> [--watch]
```

What the app holds: its resident memory and share of RAM, then per cell
its newest state's size, its event count and their arguments' size.
`--watch`: again every 2 s, a top view; `ax top <app> [secs]` (aio's
`am top`) at another interval, `--json` once. Not running: the stored
cells only.

### `ax persist`

```text
ax persist <app> [--drop cell]
```

Writes what the running app holds back now (a coalesced run of calls)
instead of when it would. `--drop cell`: a stored cell this build no
longer declares (the boot's "stored cell is not declared"): its history
is gone from the store, zeroed as `#[cell(retire)]` does; refused for a
cell the app declares.

### `ax cost`

```text
ax cost <app> [cell] [--keys] [--window s]
```

What each cell sent its clients (pushes, bytes per second, mean size, full
resends) and its methods' p95 time; its top keys by bytes (`--keys`: all).
Since its first send, or `--window 5m` over the newest 5 minutes (seconds,
or `30s`, `5m`, `1h`; an hour at most: the app keeps a meter a second that
long).
For "why is this cell slow to sync". [Performance](../performance.md).

### `ax clients`

```text
ax clients <app> [cell]
```

Without a cell: the running app's windows, each with its `--window`
number, title and page. With one: the links open to that cell, each with
who, from where, since when and its wire (`aio-rs`, `tool`, or `aio` for an
aio page or tool on protocol v3).

### `ax peer`

```text
ax peer <app> [cell…] [--as user] [--role r] [--types file]
```

What a network caller (a user, a role) may call of the running app: each
cell's methods with their Rust signatures, and what each `#[client]` rule
answers for that caller. To check the rules before a client meets them.
Each cell also says which of its fields are never sent (`#[server]`) and
never saved (`#[transient]`, or a cell kept in memory); `--json` gives them
as `fields: {persist, server, transient}`. Cells named: those only.

Another machine's app (aio's `am peer <url>`): name its profile, `ax
profile`'s `.aioapp` file, and its cells (a server lists none): `ax peer
notes.aioapp todo`. It answers for that profile's key holder, never for a
user or role asked for. `--types file` writes the answer as Rust: a module
a cell, its `ID` and a const a method (`todo::add`), each under its
signature, for `aio::peer::Peer::call`: a renamed method fails to compile.

### Common mistakes

- Reading `ax state` of an installed (release) app and missing fields: a
  release build's door shows what a client sees. Run a debug build
  (`ax dev`, `ax start` in the workspace) for the whole state.
- An unquoted path with `?` or `{`: the shell expands it first.
- Reading a stopped app's state as current: the line says `stored`, and
  how many newer events aren't in it yet.

## Drive a running app

### `ax call`

```text
ax call <app> <cell> <method> [arg…] [--args json] [--body json] [--wait s] [--as user]
```

Calls a method of the running app and prints its answer (JSON), or its
refusal (exit 1).

| Arg, flag | Does |
|---|---|
| `arg…` | the method's arguments in order: each word as JSON when it reads as JSON (`3`, `true`, `null`, `[1,2]`, `{"a":1}`), else as text |
| `key=value…` | words that are all `key=value` (aio's named payload): one object argument, each value read as a word (`title=milk n=2` is `{"title":"milk","n":2}`) |
| `--args json` | all of them exactly, as a JSON list; `@file` reads it from a file, `-` from stdin |
| `--body json` | one JSON object as the one argument (aio's `--body`) |
| `--wait s` | its answer waited for `s` seconds at most (default: until it comes); then exit 1, the call maybe still running |
| `--as user` | as that user (default: you) |

```sh
ax call notes todo add "buy milk"
ax call notes todo set_done 3 true
ax call notes todo rename --args '[3, "true"]'     # "true" as text
ax call notes todo import --args @rows.json       # too big for a command line
echo '["buy milk"]' | ax call notes todo add --args -
ax call notes todo clear                          # no arguments
ax call notes todo add_item title=milk qty=2      # one object {title, qty}
```

- A whole number too big for JSON would arrive changed, so it is refused;
  pass it as a string with `--args`.
- A text that reads as JSON (`true`, `3`) becomes JSON, and a lone text
  `a=b` an object: use `--args` with the value in quotes.
- An answer means the call ran and committed; it reaches the disk with the
  next coalesced write ([durability](../persistence/README.md)).
- On a release build only the methods a client may call are reachable.

### `ax expect`

```text
ax expect <app> <cell> <path> <op> [value] [--wait s] [--as user]
```

Asserts on the live state: `PASS …` (exit 0) or `FAIL …` (exit 1), saying
why either way. `path` as `ax state`'s.

| Op | Passes when |
|---|---|
| `eq`, `ne` | equal, not equal, as JSON (`1` and `1.0` alike, maps in any key order); the value is JSON when it reads as JSON, else text |
| `gt`, `gte`, `lt`, `lte` | a number compares so |
| `contains` | a text holds it, or a list holds the value |
| `exists` | the path is there (no value) |
| `absent` | the path is not there (no value): the one op a missing path passes |

| Flag | Does |
|---|---|
| `--wait s` | checked again after each change (pushed, not polled) until it passes, at most `s` seconds |
| `--as user` | as that user |

```sh
ax call notes todo add "buy milk"
ax expect notes todo items.0.title eq "buy milk"
ax expect notes todo items --wait 10 exists
```

A path that isn't there fails every op but `absent`: a typo is never a
pass. Each op's value count is checked before the app is asked.

### `ax replay`

```text
ax replay <app> [range] [--from file] [--dry]
```

Calls a log's inputs again on the running app, in order, to reproduce a
bug. Point it at a fresh copy (`ax dev notes --instance r1 --fresh`) to
replay a captured session.

| Arg, flag | Does |
|---|---|
| `range` | `N`, `N..M`, `N..`, `..M` (inclusive; none: all) |
| `--from file` | another store, or a backup |
| `--dry` | what would be sent, nothing sent |

Only inputs are sent: an event a schedule, an effect or the boot caused is
skipped (the running app makes it again), as is a time-travel move, and an
event written by another version of the cell than the running one. Each
skip says why. A refused call fails the replay (exit 1); the rest are
still sent, each saying how it went.

### `ax record`

```text
ax record <app> <cell> <file> [range] [--from file] [--type path]
```

A Rust test from what the app's cell did: it boots the cell on the state
before the range, makes the calls it took (inputs only, each as its
caller), and asserts the state they left. Reproduce the bug, then record.
The states come from the running app; a stopped app's store (or `--from`
another) is replayed by the app's own code, built by cargo as its tests
are, so a crash is recorded after the app is gone.

| Flag | Does |
|---|---|
| `--from file` | the events of another store or backup |
| `--type path` | the cell's type (default `<the app's library>::<Cell>`) |

```sh
ax events notes todo --last 10
ax record notes todo tests/regress.rs 30..41
ax test notes regress
```

An existing file is refused. [Tests from a running app](../testing/README.md#tests-from-a-running-app).

## The window

These reach the running app's window. A dev build's through its own door,
no flag needed; another build's through the DevTools protocol: `--cdp
port`, else `AIORT_CDP`, else the port a launch with `--cdp` opened
(loopback only).

| Flag | Does |
|---|---|
| `--cdp port` | the DevTools port to use |
| `--window n` | which of its windows (0, the first; `ax clients` numbers them) |
| `--wait-client ms` | wait that long for the window (right after a start or reload; default: none) |

Never scan the machine's ports for "the" window: with two apps up that
finds the other one's.

### `ax shot`

```text
ax shot <app> [file] [--cdp port] [--window n] [--check base] [--update base] [--threshold n] [--max-diff r] [--selector css] [--full] [--size WxH] [--video file] [--duration s] [--markers]
```

A PNG of the window (default `<id>-<time>.png`). `--json`: `{file, bytes,
width, height, did}`; with `--video`, `{"event":"recording","file","t0"}`
on stderr once its first frame is taken (`t0`: its epoch ms, the video's
clock), then the video's `{file, bytes, frames, seconds, t0, markers}`.

| Flag | Does |
|---|---|
| `--check base` | exit 1 unless it looks like `base` |
| `--threshold n` | how far a channel may move and still match (2) |
| `--max-diff r` | the share of pixels that may differ (0) |
| `--update base` | record `base` |
| `--selector css` | one element only |
| `--full` | the whole page, past the viewport |
| `--size 390x844` | laid out at that size for the shot, then as it was |
| `--video file.mp4\|.webm` | record the window until Ctrl-C, encoded by ffmpeg (without it: by the window itself, slower) |
| `--duration s` | stop the video after `s` seconds |
| `--markers` | each `ax trigger` meanwhile in `file.markers.json`: `[{t, path, action, rect}]` (ms, video px) |

```sh
ax shot notes --update tests/look/home.png
ax shot notes --check tests/look/home.png --max-diff 0.01
ax shot notes --size 390x844 --selector main --full phone.png
ax shot notes --video demo.mp4 --duration 20 --markers
```

### `ax surface`

```text
ax surface <app> [path] [--names] [--rects] [--component name] [--depth N] [--full] [--headless] [--root c] [--cdp port] [--window n] [--wait-client ms]
```

What a person can see and use in the window, one line each, nested: role,
"name", `[handle]`, state. Elements are named as `TestUi` names them: a
`data-t` handle, else a name from its role and text (`SaveButton`,
`EmailInput`, `SaveButton2`…), so a test and `ax trigger` use the same
path.

| Arg, flag | Does |
|---|---|
| `path` | that element and what is under it (`row-2`, `list/row-3`); none there: exit 1 |
| `--names` | the automatic names only, in order |
| `--rects` | where each is: `@x,y wxh` in CSS px |
| `--component name` | each element handled `name` or `name-…` (every instance: `row` is `row-1`, `row-2`), with what is under it |
| `--depth N` | `N` levels below the first shown at most (0: those alone) |
| `--full` | names whole (else cut at 40 characters, marked `…`) |
| `--headless` | no window open (a server): the app's views mounted headless on its running cells, built as its tests are |
| `--root c` | the component `--headless` mounts (`App`) |

```sh
ax surface notes
ax surface notes list --rects
ax surface notes --headless --root App
```

### `ax trigger`

```text
ax trigger <app> <path> <action> [text] [--cdp port] [--window n] [--wait-client ms]
```

Acts on the element at `path` as a person does, with real mouse and key
events:

| Action | Does |
|---|---|
| `click`, `dblclick`, `hover`, `focus`, `blur` | as named |
| `type <text>` | types, appending |
| `setValue <text>` | replaces the value |
| `clear` | empties it |
| `press <key>` | a key tap (`Enter`, `ctrl+k`, `mod+s`: `mod` is Cmd on macOS, else Ctrl) |
| `keyDown <key>`, `keyUp <key>` | hold a key, then let go (games, drags) |
| `select <value>` | picks an option |
| `check`, `uncheck` | a checkbox |
| `scroll <top[,left]>` | scrolls it |
| `dragTo <path>` | drags it onto another element |

Path `window`: a key to the window itself (a global key). With no
`--window` it acts on the window opened (or reloaded) last, as aio's
trigger does on the newest client.

```sh
ax trigger notes draft type "buy milk"
ax trigger notes AddButton click
ax trigger notes window press mod+k
```

### `ax testgen`

```text
ax testgen <app> [--out file] [--cdp port] [--window n] [--wait-client ms]
```

A typed name table of what the window shows, for the app's UI tests (aio's
`am testgen`): a `&str` const per `data-t` handle and auto name, and a
module per handle with the names inside it (`ui_names::row_1::DeleteButton`
is `"row-1/DeleteButton"`). A test takes it with `mod ui_names;` and
writes `ui.click(ui_names::SaveButton)`; after a UI change run it again,
and a test naming a renamed element no longer compiles. Default `--out`:
`tests/ui_names/mod.rs` in the app.

### `ax eval`

```text
ax eval <app> <code> [--cdp port] [--window n]
```

JavaScript in the window, as the DevTools console runs it; its value as
JSON. An expression, or statements and the last one's value (`'const f =
…; f(2)'`), top-level `await` too. Promises are awaited, a DOM node is
summarised, a throw fails (exit 1). For geometry, computed styles, a fetch
from the page's origin.

```sh
ax eval notes 'getComputedStyle(document.body).backgroundColor'
ax eval notes 'document.querySelectorAll("li").length'
```

### `ax components`

```text
ax components <app> [--renders] [--wait-client ms]
```

The running app's mounted components, nested, each with how many times it
rendered and the time those renders took. `--renders`: its last 200
renders as they happened; `--json`: all of it. A dev build only (it keeps
the render spans). To find the component that renders too often
([performance](../performance.md)).

### `ax preview`

```text
ax preview <app> <component> [props] [--html]
```

One component of the app mounted alone with the props given, and what it
shows: its surface, or its HTML (`--html`). An empty state, an error card,
a long name, without driving the app there.

- `component`: its path in the app's crate (`ui::Card`).
- `props`: as written in `rsx!` (`'name: "Ada".into(), times: 2'`).

It is a test of the app's, written for the run and removed after: the
props are type-checked by the compiler, and the component renders in the
page the app's UI tests use, no window shown.

```sh
ax preview notes ui::Card 'title: "Inbox".into(), count: 0'
```

### Common mistakes

- `ax trigger … type` on a filled field appends: `setValue` replaces.
- Right after `ax start` or a reload the window may not be ready yet: add
  `--wait-client 5000`.
- `ax components` on a release build: it keeps no render spans. Use a dev
  build.

## Build and ship

### `ax build`

```text
ax build <app> [--targets a,b] [--platforms a,b] [--list] [--sizes] [--smoke] [--deb] [--appimage] [--android] [--debug] [--windows] [--web] [--dev]
```

The app's `[build] targets` (`aio.toml`), else an installable app for this
OS: `dist/<id>-<version>-<os>-<arch>.*` (Linux `.tar.gz`, macOS zipped
`.app`, Windows one-click `.exe` and a zip).

| Flag | Does |
|---|---|
| `--targets a,b` | some of the declared targets (by label), or kinds |
| `--list` | the kinds and the declared targets |
| `--platforms a,b` | each desktop target for each of `linux`, `linux-arm64`, `windows`, `windows-arm64`, `macos`, `macos-arm64`, `host` (`[build] platforms`); `all`: every one, what this machine can't build said with why and left out (aio's `--all-platforms`) |
| `--windows` | cross-built for Windows from Linux |
| `--deb` | a Debian package (needs `[app] maintainer`) |
| `--appimage` | one portable Linux file |
| `--android` | signed APKs instead (the app's lib calls `android_app!`) |
| `--debug` | with `--android`: debug APKs, debuggable, the page in `chrome://inspect` |
| `--web` | an installable web app (PWA) in `dist/<id>-web/` instead: the app for wasm32, a manifest, icons, an offline service worker |
| `--dev` | with `--web`: a debug build that shows its errors on its page |
| `--sizes` | the release binary against its budget, and which crates take its space; packs nothing |
| `--smoke` | then start each Linux bundle, `.deb` and AppImage headless, stop it, check its log (a Windows bundle on Windows, or over `AIO_WINDOWS_SSH`) |

A macOS target built on Linux or Windows is built on a Mac over ssh
(`[build.macos] host`, else `AIO_MACOS_SSH`). Size a binary with `ax build
--sizes`, not a plain `cargo build --release`, which lacks `ax`'s link
flags. [Build](../build/README.md#build-ax-build), [targets](../build/targets.md).

```sh
ax build notes --list
ax build notes --targets desktop --smoke
ax build notes --platforms linux,windows
ax build notes --platforms all
ax build notes --web
```

`ax build` always builds anew (cargo rebuilds what changed), so aio's
`--force` has nothing to force.

### `ax smoke`

```text
ax smoke <artifact>
```

Starts a window app's Linux bundle, `.deb` or AppImage headless as a person
would, then stops it: its boot time, its window's memory (PSS of the app and
its runtime), a clean stop, nothing left running. What `ax build --smoke`
runs on each bundle.

### `ax install`

```text
ax install <app> [--appimage] [--android] [--build] [--emulator] [--device serial] [--apk file] [--no-launch]
```

Builds this OS's bundle from source and installs it for this user (Linux:
its own `install.sh`, run as a person would, nothing left aside; macOS,
Windows: says how).

| Flag | Does |
|---|---|
| `--appimage` | Linux: the AppImage, built now, in `aio-rt/install/<id>-appimage/` and the app menu; its entry unpacks it in a private `TMPDIR` (aio's installer) |
| `--android` | the newest APK for the attached phone's CPU onto it, then starts it |
| `--build` | build the APK first (it refuses one older than the sources) |
| `--apk file` | that APK |
| `--device serial` | which device (else `ANDROID_SERIAL`) |
| `--emulator` | an emulator may be the device |
| `--no-launch` | install, don't start |

### `ax lab`

```text
ax lab <app> <os> [--stop] [--boot] [--status] [--reset] [--ram size] [--cpus n] [--disk size] [--os-version v] [--apk file] [--port n]
```

The app on a real `windows`, `macos`, `linux` or `android` desktop, for a
person to click around in. Manual: no gate runs it.

- **A machine you have**: `AIO_LAB_WINDOWS`, `AIO_LAB_MACOS` or
  `AIO_LAB_LINUX` (`'[ssh options] [user@]host'`) names it. Built for it,
  copied over ssh into `aio-rs-test/lab/<id>` there, and started with its
  data, runtime and caches inside that folder. `--stop`: what runs from it
  ended, the folder removed.
- **A lab booted here** (aio's `am lab`; with no machine named, or
  `--boot`): in Docker, aio's images and container names (`aio-lab-<os>`),
  so `am lab` sees the same lab: a Windows or macOS VM (QEMU, KVM, its
  disk under `~/.cache/aio/labs/<os>`), an XFCE desktop (a container, up
  in seconds) or the Android 14 emulator. Its viewer is on a loopback port
  (`--port`, else a free one); the app's `dist/` is in it at `/shared`
  (a VM's guest: `http://host.lan:8007/`; Android: the newest signed APK,
  or `--apk`, installed over adb once it has booted). `--status`: up, and
  where; `--stop`: ended and removed (a VM's disk stays); `--reset`: its
  disk too. A VM's `--ram 8G --cpus 4 --disk 64G --os-version 11`. The
  first Windows run downloads ~8.5 GB and installs itself (~30 min); macOS
  is installed by hand in the viewer.

```sh
ax build notes --android && ax lab notes android
ax lab notes linux --status
ax lab notes windows --boot --ram 16G
ax lab notes windows --stop
```

### `ax keygen`

```text
ax keygen <app> [--rotate]
ax keygen --aio [--rotate]
```

The app's release signing key (in `$XDG_CONFIG_HOME/aio-rt/keys`, else
`~/.config/aio-rt/keys`) and the `aio.toml` lines that trust it. Kept
outside the repo; back it up, an app trusts only its key. `--rotate`: a new
one. `--aio`: aio-rs's own update key, which signs the `SHA256SUMS` of
`ax release --aio` (maintainers).

### `ax release`

```text
ax release <app> [--channel c] [--notes t] [--windows] [--android] [--appimage] [--platforms a,b] [--patch-from versions] [--dir d] [--data file] [--no-build] [--allow-dirty] [--unsigned] [--workflow]
ax release --aio [--url u] [--windows] [--dir d]
```

Builds, signs and lays out a release in `dist/<channel>/` for the app's
`[updates] url`, with patches from earlier releases.

| Flag | Does |
|---|---|
| `--channel c` | its channel |
| `--notes t` | its release notes |
| `--platforms a,b` | one for each, as `ax build`; `--windows`: Windows |
| `--android` | its signed APKs, offered to open on the device |
| `--appimage` | the AppImage (an app run from one replaces itself with it) |
| `--patch-from versions` | the earlier versions to make patches from |
| `--dir d` | in `d/<channel>/` |
| `--data file` | what it reads, from a contract file instead of asking the build |
| `--no-build` | the bundles `ax build` left in `dist/` |
| `--allow-dirty` | a build of uncommitted edits too |
| `--unsigned` | signed by a key made for it alone, no `ax keygen` needed: to look at locally (no app takes it) |
| `--workflow` | write the GitHub workflow that does it on a tag |
| `--aio` | in an aio-rs checkout: aio-rs itself prebuilt, with `install.sh` and `install.ps1` pinning each archive's SHA-256 (maintainers) |
| `--url u` | with `--aio`: where the archives will be downloaded from |

[Release and update](../build/release.md).

### `ax update`

```text
ax update <app> [--from-git <ref>]
```

The installed app's versions and update state. `--from-git <ref>`: build
that branch or tag of its git source (or its checkout) and stage it for its
next start, unsigned: a developer's path, never the update checks.

### `ax installed`, `ax remove`

```text
ax installed
ax remove <app> [--data]
```

`ax installed`: the apps installed from a bundle on this machine (id,
version, when, from which bundle, running or not). `ax remove`: takes one
away (the program, its versions, menu entry and icon), never while it
runs; its data stays unless `--data`.

### `ax backup`, `ax restore`

```text
ax backup <app> [file]
ax restore <app> <file>
```

- `ax backup`: copies the app's data, safe while it runs. Default file:
  `backups/<id>-<time>.aio-backup` in the app's home, never your working
  folder (where git would offer to commit it). It prints the restore line.
- `ax restore`: puts a backup back. The app must be closed; meanwhile
  `ax status` says `maintenance`. The data it replaces is kept aside, and
  it says where.

[Backups](../persistence/files.md#backups).

### `ax runtime`, `ax prune`

```text
ax runtime [--fetch]
ax prune [--yes] [--days N] [--keep a,b]
```

- `ax runtime`: where the shared Electron runtime is, its version;
  `--fetch`: fetch it if missing. `ax runtime --prune` is `ax prune`.
- `ax prune`: what of the shared store (Electron runtimes, about 250 MB
  each, app records) may go, as a plan. What an installed app names stays;
  of the rest, a runtime stays when something started from it in `--days`
  (30) days, it is the one this `ax` ships, it is a `--keep` one, or it is
  one aio didn't write. `--yes` (`-y`) deletes exactly what the plan names.

### Common mistakes

- Measuring a binary with `cargo build --release`: `ax build --sizes` is
  the size people get.
- Losing the signing key: installed apps trust only it. Back up the keys
  folder; `ax keygen --rotate` is a planned move, not a recovery.
- `ax restore` on a running app: refused. `ax stop` it first.

## Network and accounts

### `ax auth`

```text
ax auth <app> <do> [id] [value] [--role r] [--email e] [--password p]
```

The app's accounts, running or not.

| `do` | Does |
|---|---|
| `users` | list them: role, email, verified, second factor, locked |
| `create <id>` | add one (`--role`, `--email`, `--password`) |
| `passwd <id>` | set a password |
| `role <id> <role>` | change a role |
| `totp <id> off` | clear the second factor (a lost device) |
| `unlock <id>` | clear a lockout |
| `verify <id>` | mark the email verified |
| `revoke <id>` | end every session of the user |
| `rm <id>` | remove the account |

Without `--password` one is made and shown once.
[Accounts](../auth/README.md#accounts).

### `ax pair`

```text
ax pair <app> [--plain]
ax pair <app> <host:port> <code>
```

- `ax pair <app>` (aio's `am pair`): a fresh pairing code from the running
  app: 8 digits (6 with `--plain`), valid 3 minutes, once. A new one needs
  no restart. An app that isn't running, or has no server, is told how to
  start it or give it one.
- `ax pair <app> <host:port> <code>`: pairs this device with an app's
  server by that code. Over TLS the server's root is pinned; refused when
  the server serves another app.

Before 0.2.0 the code was `ax pin <app>`; `ax pin` is now aio's framework
version.

[Pairing](../state/sync.md#pairing-and-discovery).

### `ax profile`, `ax import`

```text
ax profile <app> [--out file]
ax import <app> <file>
```

- `ax profile`: the running app's profile (`.aioapp`): host, port, root,
  key. It is a credential: `--out` writes it readable by you alone. An app
  listening on this machine only gives a profile that connects from here
  only (said on stderr).
- `ax import`: links this device to an app by such a file, as `ax pair`
  does.

### `ax discover`

```text
ax discover [--timeout ms]
```

The aio-rs apps on this LAN that answer (`[server] discover = true`),
within `--timeout` ms (1200).

### `ax trust`

```text
ax trust [path]
```

This machine's TLS root, and how to make browsers trust it. It explains;
it installs nothing. `path`: the file's path alone.

## About ax

### `ax agent`

```text
ax agent [section] [--list] [--min] [--max]
```

The brief for coding agents: the rules first, then how aio-rs works, then
every verb. It prints text even into a pipe (the text is the answer).

| Arg, flag | Does |
|---|---|
| `section` | one section in full |
| `--list` | the sections |
| `--min` | compact: the rules, the model, the loop |
| `--max` | everything, the deep sections too |

`ax create` writes `AGENTS.md` (and a `CLAUDE.md` importing it) into each
app, pointing here, so an agent that reads no docs still finds it.
[Agents](../agents.md).

### `ax upgrade`, `ax uninstall`

```text
ax upgrade [version] [--aio dir]
ax uninstall [--tools]
```

`ax upgrade` moves `ax` itself to aio-rs's newest release (or `version`):
it takes the release's install script, checks its SHA-256 against the
release's `SHA256SUMS`, and runs it (the script checks the archive's
SHA-256 before anything runs). `--aio dir`: `ax` built from that checkout
(`cargo install`); plain `ax upgrade` returns to a release. Apps keep their
own aio-rs: [`ax pin`](#ax-pin).

`ax uninstall` takes `ax` out of `$CARGO_HOME/bin`; `--tools` also what it
fetched and built in the tool home (`tools/`, and `target/` unless
`CARGO_TARGET_DIR` names another). Apps, their data and installs stay
([setup](../basics/setup.md#uninstalling)).

### `ax feedback`

```text
ax feedback [app] [--create]
```

Where findings about aio-rs go (`AIO_FEEDBACK_DIR`, else `feedback/` in
aio's data home, shared with aio) and the reports in it; with `app`, that
app's file (`--create`: make it).

### `ax help`, `ax version`

```text
ax help [verb] [--commands]
ax version
```

`ax help` (and a bare `ax`): the everyday verbs on one screen (aio's
tiers); `--commands`: every verb with all it says; `ax help <verb>` or
`ax <verb> --help`: one in full. `ax version`: the aio-rs version of this
`ax`.

### `ax control`

```text
ax control <app> [--port n]
```

The running app's verbs over loopback HTTP, for a tool that can't run
`ax` (aio's control REST API, `/__aio/trojan/*`). A mirror of the command
line, not a second API: `GET /__aio/trojan/<verb>?arg=…&flag=v&switch`
answers what `ax <verb> <app> … --flag v --switch --json` does (`arg`: the
verb's words in order; a two-word verb: `snapshot/load`), with the status
of its exit code (200, 422 failed, 400 bad usage). The verbs about a
running app that answer once (`state`, `call`, `expect`, `timeline`,
`surface`, `trigger`, `shot`, `logs`, `health`, `stop`…; never `--follow`
or `--watch`). Only on `127.0.0.1`, only with the key it prints first,
made for the run (`Authorization: Bearer <key>`), never from a page (a
request with an `Origin`, or another `Host`, is refused). Until Ctrl-C.

```sh
ax control notes --port 7811 > key &
curl -H "Authorization: Bearer $(head -1 key)" 'http://127.0.0.1:7811/__aio/trojan/state?arg=todo&arg=items.0'
```

## Scripts and agents

Every verb answers in one call, with an exit code to branch on:

```sh
ax start notes --wait 60 || { ax errors notes; exit 1; }
ax call notes todo add "buy milk"
ax expect notes todo items.0.done eq false
ax state notes todo items --json
ax surface notes --json
ax stop notes
```

- Read `--json` answers by `ok`, then `value` or `text`.
- Run throwaway copies with `AIORT_DATA=<dir>` (and a nested display:
  `ax dev` picks one when there is no terminal), never on a person's data
  or screen.
- End apps with `ax stop`, never by process name.

## Troubleshooting

| Symptom | Cause, fix |
|---|---|
| `no app here (no Cargo workspace)` | run in the app's folder, or `--app <name>` |
| `notes isn't running (nothing was sent)` | `ax start notes`; or it runs from another home: name it (`notes@qa`, `--instance`) |
| `… isn't running from its own home but from …, …` | two other copies run: pick one with the flag it names |
| `ax state` shows fewer fields than the code has | a release build's door: run a debug build |
| `ax start` times out | a cold build or a slow boot: `--wait 120`; `ax errors notes` and `logs/start.log` say why |
| `ax status` says `stopped: crashed or killed` | `ax errors notes`, `ax report notes` |
| A verb answers about the wrong app | another workspace's app of that name: `cd` into the right one or `--app` |
| `ax shot` or `ax trigger` can't reach the window | not a dev build and no `--cdp`: start with `--cdp`, or pass `--cdp port` |
| A flag is refused | `ax help <verb>`: the gate names the flags the verb takes |

[More symptoms](../debugging/troubleshooting.md).

## Coming from aio's `am`

| `am` | `ax` |
|---|---|
| `am create --template=…` | `ax create --template app\|cli\|service` |
| `am dev`, `am build`, `am compile` | `ax dev`, `ax build`, `ax build --targets <one>` |
| `am start`, `am stop`, `am restart`, `am kill`, `am status` | the same verbs (`--wait s`, `--no-wait`) |
| `am instances` | `ax apps` |
| `am dispatch cell:method args`, `k=v`, `--body`, `--wait=N` | `ax call <app> <cell> <method> args`, `k=v`, `--body`, `--wait N` |
| `am state`, `am state --ui <user>` | `ax state`, `ax state --as <user>` |
| `am state --prod` | `ax state` on a release build (its door gives the client's view) |
| `am state --offline` | `ax state` on a stopped app (it falls back to the store) |
| `am timeline [--from=J]`, `am actions` | `ax timeline [--from file]` (a stopped app's too), `ax events` |
| `am timetravel`, `am replay`, `am record` | the same verbs (`ax record` writes a Rust test, of a stopped app too) |
| `am snapshot`, `am snapshot load --force`, `am migrations` | the same verbs |
| `am persist`, `am persist --drop=<cell>` | `ax persist`, `ax persist --drop <cell>` |
| `am logs --filter=x`, `--lines=N` | `ax logs <app> x`, `--last N` |
| `am errors --since=15m\|reload`, `--lines=N` | `ax errors --since 15m\|reload`, `--last N` |
| `am client`, `am top [secs]` | `ax components`, `ax top <app> [secs]` |
| `am cost --window=5m`, `am discover --timeout=ms` | `ax cost --window 5m`, `ax discover --timeout ms` |
| `am peer <app\|url> --types=<file>` | `ax peer <app\|profile.aioapp> --types <file>` (Rust consts) |
| `am testgen` | `ax testgen` (a Rust name table) |
| `am migrate --from=X`, `--tests` | `ax lint --from X`, `--tests` |
| `am lab <os> --status\|--stop\|--reset`, `--ram --cpus --disk --version --apk --port` | `ax lab <app> <os>` the same, `--os-version` |
| `am build --all-platforms` | `ax build --platforms all` |
| `am doctor`, `am fix --harden` | `ax doctor`, `ax doctor --harden` |
| `/__aio/trojan/*` (the control REST API) | `ax control <app>`: the verbs over loopback HTTP |
| `--quiet`, `--json` | `--quiet`, `--json` |
| `am surface` (`--component --depth --full`), `am trigger`, `am shot` (`--json`), `am eval`, `am preview` | the same verbs and flags |
| `am publish` | `ax release` |
| `am pair` (a new PIN) | `ax pair <app>`; `ax pair <app> <host:port> <code>` pairs this device |
| `am ui`, `am ui --client=browser` | `ax ui`, `ax ui --browser` |
| `am pin`, `am pin <v>\|main\|latest\|<path>`, `--latest`, `--major` | `ax pin`, `ax pin <v>\|main\|latest\|<path>`, `--major`; `ax create --aio-version` |
| `am upgrade`, `am uninstall` | `ax upgrade`, `ax uninstall` (an app's aio-rs: `ax pin`) |
| `am prune versions` | nothing to prune: an app pins aio-rs in its `Cargo.lock` |
| `am fix`, `am link` | nothing to repair: a clone builds as it is (aio-rs from git); `ax pin <v>` moves a checkout's path back to a release |
| `am check`, `am where` | the compiler: client and server code are resolved by rustc |

The full map of aio's commands and constructs: [migration](../migration/README.md).

### What `am` has that `ax` hasn't

Not in aio-rs today; each with what to do instead:

- **`am kill --stale`** (orphans serving without a lock): an aio-rs app's
  lock is the OS's, held by the process itself, so there are none to reap.
- **`am start --port`**: the port is the app's `[server] listen`.
- **`am dispatch` plain actions and `--as-server`**: there are no
  non-cell actions; a debug build's door already has the operator's rights.
- **`am pin`'s preflight** (a forward move refused while the app uses an
  API the target removed, `--force`), its Electron alignment and its
  `requires Deno ≥`: `cargo check` after `ax pin` names each removed API
  at its line; an app's Electron and Rust come with its aio-rs crates.
- **`am build --force`**: `ax build` always builds anew.
- **`am watch`, `am add cell|server`**: `ax dev` watches; cells are
  written by hand.
- **`am report`** collecting a new report from the CLI: `ax report` lists
  and shows the reports the app made.
- **Piped output as JSON by default**: `ax` prints the same text to a pipe;
  ask for `--json`.
- **amui's** V8 heap, effect backlog, per-action reduce time and action
  errors: aio-rs has no such readings; `ax ui` shows its counterparts
  ([above](#ax-ui-the-visual-app-manager)).

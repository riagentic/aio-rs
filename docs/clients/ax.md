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
| `--app <name>` | the app, wherever you are |
| `--profile <name>`, `--instance <name>`, `--home <dir>` | which copy |
| `--help` (`-h`) | this verb's help, and nothing run |
| `--version` (`-V`) | `aio-rs <version>`, and nothing run |

There is no `--quiet`: the exit code is the short answer, `--json` the
whole one.

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
  failed.
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
ax create <name> [--template t] [--id id] [--aio dir] [--force]
```

A new app in `./<name>` that runs, tests and builds as it is: its own
workspace, `aio.toml`, a test, and the pointer to the agent brief
(`AGENTS.md`, `CLAUDE.md`).

| Flag | Does |
|---|---|
| `--template app` | a window app (the default) |
| `--template cli` | a command-line tool ([cli apps](cli-apps.md)) |
| `--template service` | a served app, no window |
| `--id id` | its id, its data's home (default: the name; refused when an installed app has it) |
| `--aio dir` | the aio-rs checkout it builds on (default: the one this `ax` was built from; a prebuilt `ax` needs it) |
| `--force` | write the template over a folder that holds files |

```sh
ax create notes
cd notes
ax test notes && ax dev notes
```

[Quickstart](../basics/quickstart.md), [the files it makes](../basics/project.md).

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
ax lint [dir] [--json] [--no-hints|--quiet-hints] [--no-clippy] [--safe-fix] [--fix]
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

Exit 0 with no errors (warnings and hints don't fail), 1 with errors, 2 on
bad usage. Silence one finding, with its reason, on its line or in the
comment block above: `// aio-ok: <why>` or `// aio-ok(<area>): <why>`. A
silence without a reason is itself a finding. [Lint](../testing/README.md).

### `ax start`

```text
ax start <app> [arg…] [--wait s] [--fresh] [--display d] [--cdp] [--all]
```

Runs the app in the background, detached from your terminal (its own
process group: a Ctrl-C there doesn't reach it), and waits until it is
up: its lock held, its `health.json` written by this start, and, for a
window app, its window ready. In a workspace it is built first; elsewhere
the installed one runs.

| Flag | Does |
|---|---|
| `--wait s` | wait at most `s` seconds for it to be up (30) |
| `--fresh`, `--display d`, `--cdp` | as `ax dev` |
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
ax stop <app> [app…] [--all] [--wait s]
```

Asks the app to close, on any OS, over its local door, and waits until it
is gone (its lock free: the OS drops it with the process, so "gone" is
never a guess). It runs its own shutdown: stops, the last write, the
window closed.

| Flag | Does |
|---|---|
| `--wait s` | how long asking may take (15); then SIGTERM, then SIGKILL, 5 s each |
| `--all` | every running app of this workspace; the rest are said |

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
ax doctor <app>
```

Is the running app the code on disk? A process keeps the code it started
with: a rebuild or an edit since is a finding, with its fix (stop it and start it again). It also says when the app runs
another aio-rs than this `ax`. Then who decided each setting it runs with: `aio.toml`, the
folders, the environment variables aio reads (a token's value is never
shown).

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
ax snapshot load <app> <file>
```

- `ax snapshot`: every cell's state as one JSON object (live per cell, else
  stored; `null` for a cell with no snapshot yet).
- `ax snapshot save`: that object as a new file (default
  `<id>-snapshot.json`; an existing file is refused, never overwritten).
- `ax snapshot load`: each cell the file holds put in place on the running
  app, each as a new event, and it says how to step back (`ax timetravel
  notes todo goto 41`). Cells the file doesn't hold are left as they are. A
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
| `--diff` | each with the changes it made, `path: before → after` (one cell, the app running) |
| `--from file` | another store, or a backup file |

```sh
ax events notes todo --last 5 --diff
```

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
ax errors <app> [--last N]
```

`ax dev`'s failed build first (the app runs its last good build meanwhile),
then the newest warnings, errors and panics (20), each under the start of
the app it came from. The first thing to run when something broke.

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

### `ax heap`

```text
ax heap <app> [--watch]
```

What the app holds: its resident memory and share of RAM, then per cell
its newest state's size, its event count and their arguments' size.
`--watch`: again every 2 s, a top view. Not running: the stored cells
only.

### `ax cost`

```text
ax cost <app> [cell] [--keys]
```

What each cell sent its clients (pushes, bytes per second, mean size, full
resends) and its methods' p95 time; its top keys by bytes (`--keys`: all).
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
ax peer <app> [--as user] [--role r]
```

What a network caller (a user, a role) may call of the running app: each
cell's methods with their Rust signatures, and what each `#[client]` rule
answers for that caller. To check the rules before a client meets them.
Each cell also says which of its fields are never sent (`#[server]`) and
never saved (`#[transient]`, or a cell kept in memory); `--json` gives them
as `fields: {persist, server, transient}`.

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
ax call <app> <cell> <method> [arg…] [--args json] [--as user]
```

Calls a method of the running app and prints its answer (JSON), or its
refusal (exit 1).

| Arg, flag | Does |
|---|---|
| `arg…` | the method's arguments in order: each word as JSON when it reads as JSON (`3`, `true`, `null`, `[1,2]`, `{"a":1}`), else as text |
| `--args json` | all of them exactly, as a JSON list; `@file` reads it from a file, `-` from stdin |
| `--as user` | as that user (default: you) |

```sh
ax call notes todo add "buy milk"
ax call notes todo set_done 3 true
ax call notes todo rename --args '[3, "true"]'     # "true" as text
ax call notes todo import --args @rows.json       # too big for a command line
echo '["buy milk"]' | ax call notes todo add --args -
ax call notes todo clear                          # no arguments
```

- A whole number too big for JSON would arrive changed, so it is refused;
  pass it as a string with `--args`.
- A text that reads as JSON (`true`, `3`) becomes JSON: use `--args` with
  the value in quotes.
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

A Rust test from what the running app's cell just did: it boots the cell
on the state before the range, makes the calls it took (inputs only), and
asserts the state they left. Reproduce the bug, then record.

| Flag | Does |
|---|---|
| `--from file` | the events of another store or backup |
| `--type path` | the cell's type (default `<crate>::<Cell>`) |

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

A PNG of the window (default `<id>-<time>.png`).

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
ax surface <app> [path] [--names] [--rects] [--headless] [--root c] [--cdp port] [--window n] [--wait-client ms]
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

Path `window`: a key to the window itself (a global key).

```sh
ax trigger notes draft type "buy milk"
ax trigger notes AddButton click
ax trigger notes window press mod+k
```

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
| `--platforms a,b` | each desktop target for each of `linux`, `linux-arm64`, `windows`, `windows-arm64`, `macos`, `macos-arm64`, `host` (`[build] platforms`) |
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
ax build notes --web
```

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
ax install <app> [--android] [--build] [--emulator] [--device serial] [--apk file] [--no-launch]
```

Builds this OS's bundle from source and installs it for this user (Linux:
its own `install.sh`, run as a person would, nothing left aside; macOS,
Windows: says how).

| Flag | Does |
|---|---|
| `--android` | the newest APK for the attached phone's CPU onto it, then starts it |
| `--build` | build the APK first (it refuses one older than the sources) |
| `--apk file` | that APK |
| `--device serial` | which device (else `ANDROID_SERIAL`) |
| `--emulator` | an emulator may be the device |
| `--no-launch` | install, don't start |

### `ax lab`

```text
ax lab <app> <os> [--stop]
```

The app on a real `windows`, `macos` or `linux` machine's desktop, for a
person to click around in. The machine is `AIO_LAB_WINDOWS`,
`AIO_LAB_MACOS` or `AIO_LAB_LINUX` (`'[ssh options] [user@]host'`). Built
for it, copied over ssh into `aio-rs-test/lab/<id>` there, and started
with its data, runtime and caches inside that folder. `--stop`: what runs
from it ended, the folder removed.

`android` is aio's container instead (`am lab android`): Google's emulator
(`budtmo/docker-android`, docker with KVM), its screen in a browser at
`http://127.0.0.1:6080` (loopback only); the app's x86_64 APK is built,
installed and started in it once it has booted (1 to 3 minutes the first
time). `--stop` removes the container.

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

### `ax pin`, `ax pair`

```text
ax pin <app> [--plain]
ax pair <app> <host:port> <code>
```

- `ax pin`: a pairing code from the running app: 8 digits (6 with
  `--plain`), valid 3 minutes, once. A new one needs no restart.
- `ax pair`: pairs this device with an app's server by that code. Over TLS
  the server's root is pinned; refused when the server serves another app.

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
ax discover
```

The aio-rs apps on this LAN that answer (`[server] discover = true`).

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

### `ax feedback`

```text
ax feedback [app] [--create]
```

Where findings about aio-rs go (`AIO_FEEDBACK_DIR`, else `feedback/` in
aio's data home, shared with aio) and the reports in it; with `app`, that
app's file (`--create`: make it).

### `ax help`, `ax version`

```text
ax help [verb]
ax version
```

Every verb, or one in full; the aio-rs version of this `ax`.

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
| `am start`, `am stop`, `am restart`, `am kill`, `am status` | the same verbs (they always wait; `--wait s` bounds it) |
| `am instances` | `ax apps` |
| `am dispatch cell:method args` | `ax call <app> <cell> <method> args` |
| `am state`, `am state --ui <user>` | `ax state`, `ax state --as <user>` |
| `am state --prod` | `ax state` on a release build (its door gives the client's view) |
| `am state --offline` | `ax state` on a stopped app (it falls back to the store) |
| `am timeline`, `am actions` | `ax events --diff`, `ax events` |
| `am timetravel`, `am replay`, `am record` | the same verbs (`ax record` writes a Rust test) |
| `am snapshot`, `am migrations` | the same verbs |
| `am logs --filter=x` | `ax logs <app> x` |
| `am client`, `am top` | `ax components`, `ax heap --watch` |
| `am surface`, `am trigger`, `am shot`, `am eval`, `am preview` | the same verbs |
| `am publish` | `ax release` |
| `am pair` (a new PIN) | `ax pin`; `ax pair` pairs this device |
| `am ui`, `am ui --client=browser` | `ax ui`, `ax ui --browser` |
| `am pin`, `am upgrade`, `am prune versions` | the app's `Cargo.lock` pins aio-rs: `cargo update -p aio-rs` |
| `am fix`, `am link` | nothing to repair: a clone builds with `cargo` (`ax lint --fix` moves an older layout) |
| `am check`, `am where` | the compiler: client and server code are resolved by rustc |

The full map of aio's commands and constructs: [migration](../migration/README.md).

### What `am` has that `ax` hasn't

Not in aio-rs today; each with what to do instead:

- **`am persist`, `am persist --drop`**: no verb forces a write or drops a
  stored cell the build no longer declares. Writes are coalesced and flushed
  by `ax stop`; an undeclared cell's data is kept (`ax migrations` lists it).
- **`am snapshot load --force`** and its refusal of another app's snapshot:
  `ax snapshot load` loads each cell the file holds and leaves the others.
- **`am dispatch` extras**: `key=value` pairs as one object argument (pass
  the object as JSON), `--body` raw envelopes, plain non-cell actions, and
  `--as-server` (a debug build's door already has the operator's rights).
- **`am kill --stale`** (orphans serving without a lock): an aio-rs app's
  lock is the OS's, held by the process itself, so there are none to reap.
- **`am start --no-wait`, `--port`**: `ax start` always waits (bound it with
  `--wait`); the port is the app's `[server]` setting.
- **`am errors --since=reload`**: `ax errors` groups by start, not by
  hot reload.
- **`am surface --component`, `--depth`, `--full`**: `ax surface` takes a
  path, `--names` and `--rects`.
- **`am peer --types`**: `ax peer` prints the signatures; no types file.
- **`am testgen`** (a typed test client): write `TestUi` tests by handle.
- **`am migrate`, `am migrate --tests`** (retired APIs an app still uses):
  the compiler names them.
- **`am watch`, `am add cell|server`**: `ax dev` watches; cells are written
  by hand.
- **`am report`** collecting a new report from the CLI: `ax report` lists
  and shows the reports the app made.
- **The control REST API** (`/__aio/trojan/*`) and `--quiet`: tools reach a
  running app over its local door only; use `--json` and exit codes.
- **amui's** V8 heap, effect backlog, per-action reduce time and action
  errors: aio-rs has no such readings; `ax ui` shows its counterparts
  ([above](#ax-ui-the-visual-app-manager)).

# Files, backups and restore

Where an app's data lives, what to back up, and how to put it back. An
app finds its folders with `aio::paths::app_dirs()` (the dirs it booted
on; a test's own sandbox under `TestCells` or `TestUi`), so app code never
builds a path from the OS's folders itself.

## Where files live

One app, one home. Everything the app owns is in it; what lives outside
is either made again or belongs to every app ([below](#outside-the-apps-home)).

| | Linux | macOS | Windows |
|---|---|---|---|
| the app's home | `~/.local/share/aio-rt/apps/<id>/` | `~/Library/Application Support/aio-rt/apps/<id>/` | `%LOCALAPPDATA%\aio-rt\apps\<id>\` |
| its cache | `~/.cache/aio-rt/<id>/` | `~/Library/Caches/aio-rt/<id>/` | `%LOCALAPPDATA%\aio-rt\cache\<id>\` |
| its sockets | `$XDG_RUNTIME_DIR/aio-rt/<id>-<hash>/` (else the temp dir; `/tmp/aio-rt-<uid>/…` when too long for a socket) | the temp dir, `aio-rt/<id>-<hash>/` (`/tmp/aio-rt-<uid>/…` when too long) | the temp dir, `aio-rt\<id>-<hash>\` |

`XDG_DATA_HOME` and `XDG_CACHE_HOME` move the Linux ones as usual.

```text
apps/<id>/
  data/      ① back this up: nothing here can be made again (0700)
  logs/      ② app.log, error.log, warning.log, update.log, rotated (0700)
  backups/   what `ax backup` writes when given no file
<cache>/<id>/  ② made again as needed: downloads, thumbnails, build trees
<sockets>/     ③ the running app's doors: gone with it
```

`ax data <app>` prints each folder with its size, whether the app runs,
and who chose the folders:

```sh
ax data notes
ax data notes --json
```

| Tier | Lose it and… | Backed up |
|---|---|---|
| ① `data/` | the data is gone | yes, all of it but the files [below](#backups) |
| ② `logs/`, the cache | it is made again (logs start over) | never |
| ③ the sockets folder | nothing: it lives as long as the run | never |

The sockets are apart on purpose: a Unix socket path must stay under
about 100 bytes, which a deep home would pass, and a socket must not
outlive the run.

### What's in data/

| File | What | Secret |
|---|---|---|
| `state.db` (`-wal`, `-shm`) | the cells: their log, snapshots, schedules, `#[table]` rows ([persistence](README.md#how-the-store-works)); `[data] store` renames it | the log holds every call's arguments |
| `state.db.good` | the store's daily good copy, booted from if the store is damaged | as `state.db` |
| `state.db.v<old>-<seq>.bak` | the store before a cell's shape version changed | as `state.db` |
| `state.db.damaged-<ms>`, `.replaced-<ms>` | a damaged store set aside, a store a restore replaced: never deleted for you | as `state.db` |
| `auth.db` | accounts and sessions ([accounts](../auth/README.md)) | yes |
| `files/` | the app's own files (`AppDirs::files()`), blobs in `files/blobs/` ([blobs](sql.md#blobs-files-and-big-data)) | yours to say |
| `copy.json` | a `client`-mode window's copy of its server's view and its calls not yet sent ([offline](offline.md)) | as the state |
| `sync.token`, `paired.json` | the key a served app's clients show, a paired device's link | yes |
| `serving.json` | where the running app serves (made per run) | |
| `health.json` | the running app's health (made per run) | |
| `instance.lock` | the single-instance lock (held while it runs) | |
| `reports/` | problem reports the app made | may hold state |
| `electron/` | the window's browser profile: cookies, local storage | yes |
| `window.json` | the window's last size and place | |
| `backups/` | the store before each update (`pre-<version>-<stamp>.db`, the newest 3 kept) | as `state.db` |
| `updates.db`, `update-keys.json` | the updater's own state and the keys it trusts ([build](../build/README.md)) | |

`data/` and `logs/` are made 0700, and files with keys 0600. On Windows
the folder's ACL under the user's profile is what protects them.

### Outside the app's home

| What | Where | Lose it and… |
|---|---|---|
| an installed app's program | `aio-rt/install/<id>/` (`<id>-appimage` for one run from an AppImage) | install it again; the data stays |
| this machine's root for TLS | `aio-rt/tls/` (`ax trust` says how browsers trust it) | a new root is made; paired devices pair again |
| the Electron runtime windows open in | aio's Electron store, shared with aio's apps: `aio/electron/<version>-<os-arch>/` in the OS's data home ([desktop](../clients/desktop.md#one-store-with-aio)) | it is fetched again |

None of these is backed up by `ax backup`, and none holds app data.

## The program and its data

The program and its data are apart, so updating or removing one never
touches the other. `ax remove <app>` takes away an installed app (not
while it runs) and keeps its home, saying where; `ax remove <app> --data`
removes the home too, and nothing else implies it. An update backs the
store up into `data/backups/` before it installs, and an update that
fails to start rolls back to it ([build](../build/README.md)).

## Moving it

| Ask | Where the app runs |
|---|---|
| `AIORT_DATA=<dir>` | `<dir>/data`, `<dir>/logs`, `<dir>/cache` (tests, throwaway runs, a portable install) |
| `--home=<dir>` | that folder (its `data/`, `logs/`, `cache/`) |
| `--profile=<name>` (or `AIORT_PROFILE`) | a second home beside the first, `<home>-<name>`; a path is taken as a folder |
| `--instance=<name>` | a private world of its own, `aio-rt/instances/<name>/<id>` (agents, tests) |
| `--takeover` | the same home: the running app is asked to stop, and this one takes its place |

```sh
ax data notes --profile=work      # where the "work" profile keeps its data
ax doctor notes                   # who chose the folders, and every other setting
```

- Precedence: `--home`, then `--profile`/`AIORT_PROFILE`, then
  `--instance`, then `AIORT_DATA`, then the default. `--home` and
  `--profile` naming two different folders is refused.
- `--instance` takes a plain name (`agent1`), never a path.
- A home that is a file, or a folder holding another program's files, is
  refused before anything is made in it.
- The flags are aio's own, taken from a launch before the app's argument
  parser sees the rest (`aio::paths::Ask::app_args`), so the app's own
  flags never clash. `aio::paths::Ask::parse` reads them.
- Holding the folders holds the app's single-instance lock: one process
  writes its store and log. A second launch of a window app hands its
  arguments to the running one, which comes to the front. The OS lets
  go of the lock when the process ends, so a crash leaves none stale.
- aio-rs apps live under `aio-rt`, never in aio's folders, so an aio app
  and its aio-rs build run side by side. A ported app
  (`[app] port_of = "<aio id>"`) is the exception: it runs on its aio
  home (`~/.<aio id>`, aio's `--home`, `--profile`, `AIO_PROFILE`,
  `AIO_APPS_DIR` and `[home]` rules) and its `state.db`
  ([data](../migration/data.md)).

## Backups

```sh
ax backup notes notes.bak         # a consistent copy, safe while it runs
ax restore notes notes.bak        # with the app closed; what it replaces is kept aside
ax snapshot notes                 # every cell's state as one JSON object, printed
ax snapshot save notes            # the same, as a new file (notes-snapshot.json)
ax snapshot load notes notes-snapshot.json   # each cell put back as a new event
```

- `ax backup <app> [file]` (`aio::backup::backup_all`) copies the store
  with `VACUUM INTO` and the rest of `data/` (each other SQLite file
  copied consistently, its WAL in) into one file stamped with the app's
  id. Without a file it writes `<home>/backups/<id>-<stamp>.aio-backup`.
  It never overwrites a file, and it is written as `.partial` and renamed
  when whole.
- A backup leaves out what belongs to one machine or one run, or is made
  again: `instance.lock`, `health.json`, `serving.json`, `sync.token`,
  `paired.json`, the `electron/` profile, `data/backups/`, the store's own
  copies (`state.db.*`), temp files and data a restore set aside.
- `ax restore <app> <file>` refuses another app's backup and a running
  app (close it first: `ax stop <app>`). The store it replaces is kept as
  `state.db.replaced-<ms>`, and every other file it replaces, or that the
  backup didn't hold, is set aside the same way: `data/` is then what was
  backed up, nothing newer mixed in. A path in the file that would leave
  `data/` is refused.
- A snapshot is state only (no history). One taken `--as` someone holds
  only what they saw; loading one is undoable
  (`ax timetravel <app> <cell> goto <seq>` steps back).
- Store-only copies are made for you: the daily good copy, one before a
  shape change, one before each update. Only `ax backup` holds the rest
  of `data/`.
- Code that writes its own files: `aio::paths::write_atomic(path, bytes,
  private)` writes a temp file, syncs and renames, so a crash leaves the
  old file or the new one, never half of one (`private`: mode 0600);
  `write_private` makes a new owner-only file. A crash's leftover temp
  files beside the store are swept at the next boot.

### Scheduled backups

`[data] backup_every = "1d"` makes a whole backup (as `ax backup` does)
into `<home>/backups` as `<id>-auto-<time>.aio-backup` while the app
runs, keeping the newest `backup_keep` (default 7); backups made by hand
are never removed. The clock runs from the newest timed backup, so a
restart doesn't put one off; the first is made at the first launch. A
failed one is said in the log and tried again a turn later. Outside the
app, run `ax backup` from the OS's scheduler, or call
`aio::backup::timed_once(id, &dirs.store, dir, keep)` yourself.

## Secrets in files

`state.db` holds every call's arguments for as long as its log is kept
([history](README.md#history)). A secret passed to a method (a
passphrase) stays in the log unless the method is `#[redact]`, and a
credential-named field of a shared cell must be `#[server]`
([accounts](../auth/README.md#secrets-in-state)). The app's key and the
accounts' hashes stay in `data/` (owner-only); backups carry `auth.db`,
so keep backups as private as the data.

## Tests

`TestCells`, `TestUi` and `TestServer` give each test its own sandbox:
`app_dirs()` answers `<sandbox>/app`, made on first ask and removed with
the test, and the home and every OS folder aio-rs would use are inside
the sandbox. While one lives, opening app data under the real home is
refused (a temp dir is fine), so no test reaches the person's apps or
data (`aio::paths::in_sandbox()` says which world runs). For a folder a
test controls, open the store on it:
`Handle::<C>::open(dir.join("state.db"))`, or `AppDirs::at(root, id)`
for a whole home.

## Limits, compared with aio

- No WAL-checkpoint setting: SQLite checkpoints on its own.
- A whole backup reads each file of `data/` into memory as it copies it:
  a `files/` folder of many gigabytes makes a backup that large and slow.
  Back very large blob stores up with your own tools.
- No `launch.json` or `ax restart` replaying the last launch's flags.

## Common mistakes

- Building paths from `dirs`/`HOME` in app code: a test or a profile then
  writes to the real home. Ask `aio::paths::app_dirs()`.
- Backing up by copying `state.db` while the app runs: the copy may miss
  the WAL. Use `ax backup`.
- Restoring over a running app: refused; close it first (`ax stop`).
- Deleting the `.replaced-` or `.damaged-` files right away: they are
  the data a restore or a recovery set aside. Delete them once nothing
  is missing.
- Putting regenerable bulk in `data/`: it then rides every backup. Use
  `app_dirs()?.cache`.

## API

`cargo doc -p aio-rs --no-deps --open`: `aio::paths` (`app_dirs`,
`AppDirs`, `Ask`, `FLAGS`, `plan`, `write_atomic`, `write_private`,
`in_sandbox`, `sockets_at`), `aio::backup` (`backup`, `backup_all`,
`restore`, `left_out`, `app_of`, `timed`, `timed_once`).

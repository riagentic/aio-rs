# Release and update

An app updates itself from signed releases: `ax release` builds, signs
and lays out a release; the installed app finds it, checks its
signature and whether it can read the data here, stages it and installs
it at the next start, with a way back if the new version fails to start.

- **Use it** for any app people install from a bundle (Linux, macOS,
  Windows), and for servers and command-line tools run by a supervisor.
- **Not for** apps run from source (`ax dev`, `cargo run`: an update
  installs only into an installed copy), web builds (the host serves the
  newest files) or APKs (Android installs them; an Android release is
  offered as a link to open, see [Android](#android)).

## The rule above the others

**Data written by a newer version is never handed to an older one, and
never thrown away.** Every release says which data it can read; a release
that can't read the data here is not installed by itself
([the data gate](#the-data-gate)), and when an install fails the version
before comes back with its data ([the way back](#install-and-the-way-back)).

## The short version

```sh
ax keygen notes                   # once: the signing key, and the aio.toml lines to add
ax release notes --channel stable --notes "Faster lists"
# upload dist/stable/ to [updates] url, or let the GitHub workflow do it
```

```toml
[app]
id = "notes"
version = "1.4"                          # ax numbers builds 1.4.<commit count>

[updates]
url = "https://example.org/notes"        # <url>/<channel>/<os>-<arch>.json
keys = ["ed25519:REPLACE_WITH_THE_KEY_AX_KEYGEN_PRINTS"]
channel = "stable"
every = "6h"
auto = true
keep = 3

[capabilities]
network = ["example.org"]                # the hosts the app may reach
```

The `[updates] url` is all it takes: ax's builds (`ax build`, `ax
release`, `ax dev`) turn on the shell's updater for an app that has one,
and an app without one carries none of it. A plain `cargo build` of such
an app needs `--features aio-shell/updates`; without it the app says at
start that it never looks for a release.

## Every `[updates]` key

| Key | Default | Means |
|---|---|---|
| `url` | none: updates off | where releases are ([sources](#where-releases-come-from)) |
| `keys` | `[]` | the release keys the app trusts, `ed25519:<base64>` from `ax keygen`; a git source's may be `ssh-ed25519 …` lines |
| `channel` | `"stable"` | the channel followed ([channels](#channels-and-checks)): letters, digits, `-`, `_`, `.`, at most 64 |
| `every` | `"6h"` | how often a running app looks; `dev` at least every minute and `test` every 5 minutes; `"off"`: only when the app asks (`check`) |
| `every_by_channel` | `{}` | `every` per channel, over the above: `{ beta = "5m", nightly = "off" }` |
| `auto` | `true` | download in the background, install at the next start (a server: at once); `false`: only offer it |
| `prerelease` | unset | offer prereleases (`1.5.0-beta.1`); unset: only off `stable`, or to an app on a prerelease already |
| `keep` | `3` | installed versions kept, the running one included (at least 2: a rollback needs the one before) |
| `sign_commit` | `false` | every `ax release` signs in the commit it was built from (`--commit`) |

A misspelt key, a `url` that is neither https, loopback http,
`github:` nor `git+…`, or a host missing from `[capabilities] network`
fails the build, naming the fix.

## Keys

- `ax keygen <app>` makes the app's Ed25519 signing key in
  `$XDG_CONFIG_HOME/aio-rt/keys/<id>.key` (else `~/.config/aio-rt/keys`,
  mode 0600, never in the repo) and prints the public key for
  `[updates] keys`.
- The app trusts only the keys built into it (`[updates] keys`), never
  the first key a server shows: whoever controls the download host but
  not the key can't make an app take a release.
- **Rotate before you must.** `ax keygen <app> --rotate` makes a new key
  and keeps the old one as `<id>.previous.key`. Put both public keys in
  `[updates] keys` and release. Each release carries a **roster**:
  `aio.toml`'s keys, signed by every key on the releasing machine. An
  installed app that trusts any key that signed the roster trusts the
  roster's keys from then on (kept in its data as `update-keys.json`,
  and said: `release keys updated by 1.5.0's signed roster`). Copies that
  never ran the rotating release still trust the old key, so keep it until
  they have.
- **Losing the key strands every installed copy**: no release they will
  take can be made again. Back the key file up somewhere safe; rotate
  while you still have it.
- A key that leaked: rotate to a new one and release at once; the roster
  drops the leaked key for every copy that updates.
- `ax release --unsigned` signs with a key made for that one release, to
  look at locally; no installed app takes it.

## What the signature covers

A release's manifest, `<os>-<arch>.json` (an AppImage's:
`<os>-<arch>.appimage.json`), holds the release, its key and the
signature. **Every field** of the release is signed, in a fixed order:

| Field | Is |
|---|---|
| `format` | the manifest format (1) |
| `id`, `channel`, `os`, `arch` | which app, channel and platform it is for |
| `version` | its version |
| `file`, `sha256`, `size` | the bundle beside the manifest, its SHA-256 and size |
| `releasedAt`, `notes` | when, and the notes the app shows |
| `data` | what it can read: the store's layout and each cell's data version |
| `roster` | the keys to trust from this release on, signed by keys trusted already |
| `minFrom` | the oldest version it installs over (`--min-from`); left out when none |
| `commit` | the commit it was built from, with `--commit` or `[updates] sign_commit = true` (from the build's `dist/<id>.manifest.json`); its build number is the version's third part |

A manifest with `minFrom` or `commit` is read by installs on aio-rs 0.3
or newer; an install on 0.2 refuses a field it doesn't know, so a plain
release names neither. Such an install can't update at all while the
channel's newest release names either: first publish a plain release
built on aio-rs 0.3 (no `--commit`, no `--min-from`, `sign_commit` off)
as a bridge, and wait for those installs to take it. `ax release` warns,
in one line, each time a release names either. From 0.3 an install
checks the signature over the release exactly as it was signed, then reads past a field it doesn't
know: a later `ax release`'s fields never strand it, and a field added
after signing still breaks the signature.

An app checks, in order: the format; that the release is for this app,
channel, OS and architecture (`it is for app todo, not notes`); that the
version and file name are plain names (letters, digits, `.`, `-`, `_`,
`+`, not starting with `.`: no paths); that `minFrom`, if any, is a
version; the roster, if any; that the key
is trusted; the signature. Then, once downloaded, the bundle's size and
SHA-256. Any miss refuses the release (`release refused: …`), said in the
log and to the `updates` cell.

```rust
use aio::update::{Contract, Expect, Release, check, keygen, roster, sign};

# fn main() {
let (key, public) = keygen().unwrap();
let release = Release {
    format: aio::update::FORMAT,
    id: "notes".into(),
    version: "1.5.0".into(),
    channel: "stable".into(),
    os: "linux".into(),
    arch: "x86_64".into(),
    file: "notes-1.5.0-linux-x64.tar.gz".into(),
    sha256: "ab".repeat(32),
    size: 2_800_000,
    released_at: "2026-11-01T09:00:00.000Z".into(),
    notes: "Faster lists".into(),
    data: Contract { schema: 1, cells: [("notes".to_string(), 2)].into() },
    roster: None,
    min_from: None,
    commit: Some("9f3ac2b1".into()),
};
let expect = Expect { id: "notes", channel: "stable", os: "linux", arch: "x86_64" };
let m = sign(release.clone(), &key).unwrap();
assert!(check(&m, &[public.clone()], &expect).is_ok());

// One field changed after signing: refused.
let mut forged = m.clone();
forged.release.notes = "Now with a free gift".into();
assert_eq!(check(&forged, &[public.clone()], &expect).unwrap_err(), "its signature is wrong");

// Signed by a key the app doesn't have built in: refused.
let (other, _) = keygen().unwrap();
let stranger = sign(release.clone(), &other).unwrap();
assert!(check(&stranger, &[public.clone()], &expect).unwrap_err().contains("doesn't trust"));

// Rotation: the old key vouches for the new set, so the new key is taken.
let (next, next_public) = keygen().unwrap();
let rotated = Release { roster: Some(roster(&[next_public.clone()], &[key]).unwrap()), ..release };
let trusted_after = check(&sign(rotated, &next).unwrap(), &[public], &expect).unwrap();
assert_eq!(trusted_after, [next_public]);
# }
```

## `ax release`

| Flag | Does |
|---|---|
| `--channel c` | the channel it is for (`[updates] channel` by default): `dist/<c>/` |
| `--notes t` | the release notes the app shows |
| `--platforms a,b`, `--windows`, `--appimage` | the platforms released, as `ax build` |
| `--android` | its signed APKs too, offered to open on the device |
| `--patch-from 1.4.10,1.4.11` | patches from those releases (their bundles kept in `dist/<c>/`) |
| `--min-from 1.4.10` | installs only over 1.4.10 or newer ([stepping stones](#stepping-stones)); needs installs on aio-rs 0.3 or newer |
| `--commit` | signs in the commit it was built from (`[updates] sign_commit = true`: always); needs installs on aio-rs 0.3 or newer |
| `--dir d` | lays the channel out in `d/<c>/` |
| `--data file` | what this release reads, from a contract file, instead of asking the build |
| `--no-build` | releases the bundles `ax build` left in `dist/` |
| `--allow-dirty` | a build of uncommitted edits too (its version says `-dirty.<hash>`) |
| `--unsigned` | a key made for this release only: to look at, never taken |
| `--workflow` | writes the GitHub workflow that does this on a tag |
| `--pages`, `--web` | with `--workflow`: the channel deployed to GitHub Pages too; the app's web build at the site's root ([below](#github-pages)) |

What it does: builds the bundle, starts it once to ask which data it
reads (the cells it opens and their versions, the store's layout),
signs the manifest, and lays out in `dist/<channel>/` the bundle,
`<os>-<arch>.json` and, with `--patch-from`, the patches and
`<os>-<arch>.patches.json`. It ends by saying where to put them.

### Publishing

- **A web host.** Upload `dist/<channel>/` to `<url>/<channel>/`,
  **files before manifests**: an app that reads a manifest whose bundle
  isn't there yet fails its download (and tries again later).
- **GitHub releases** (`url = "github:<owner>/<repo>"`). `stable` reads
  the repository's latest release; any other channel reads the release
  tagged with the channel's name (`beta`). `ax release` prints the
  `gh release create` (or `gh release upload --clobber`) line.
- **The workflow.** `ax release <app> --workflow` writes
  `.github/workflows/release-<app>.yml` ([below](#github-workflow)).
- **GitHub Pages.** `--workflow --pages` deploys the channel there too
  ([below](#github-pages)).

A release of a version already laid out in that folder with other bytes
is refused: an install that has the version is never offered another
build of it (an update needs a newer version). A new build is a new
version: a commit raises the build number (`git commit --allow-empty -m
rebuild` when nothing changed but the build). The same bytes again go
over it. aio offers "the same version, new build" by comparing digests,
which an install that never recorded one can't see, and a stale copy of
the older build can offer back; here every build reaches every install
as the next version, and the version alone orders them.
- An app caches the manifest with its ETag, so a check that finds nothing
  new costs a `304`.

## Where releases come from

| `[updates] url` | The app looks at |
|---|---|
| `https://host/path` | `https://host/path/<channel>/<os>-<arch>.json` (a loopback `http://127.0.0.1` for tests) |
| `file:///mnt/releases/notes` | the same files in a folder: a mounted share, a USB stick, a CI artifact (Windows: `file:///C:/releases/notes`) |
| `github:<owner>/<repo>` | that repository's GitHub releases, as above |
| `git+https://…`, `git+ssh://…`, `git+file://…` | a repository whose `channel` branch or tag is built on this machine when its commit or tag is signed by one of `keys` (OpenSSH `ssh-ed25519` lines allowed) |

`[capabilities] network` must allow the host (for `github:`, GitHub's
download hosts too): an `aio.toml` that leaves it out is refused as the
app builds, naming the entry to add.

A git source suits a machine that builds its own apps: each new commit of
the channel's branch is cloned into a throwaway folder, built only if
git verifies its signature (or its tag's) against the keys, asked what
data it reads, and staged like a download. What each commit came to is
kept, so it is built or refused once.

A folder source is checked as a host is: the manifest's signature, then
the bundle's size and SHA-256. It needs no `[capabilities] network` entry.

### Other delivery

When releases reach the app some other way (a store, your own server),
keep the `updates` cell and bring the I/O: `aio::update::install(runtime)`
with your own `aio::update::Runtime` (`check` returns what it found,
`apply` installs), before `aio_shell::start`. The cell opens on it, with
or without `[updates] url`, and the built-in checks never run beside it
(a `url` too is said once at start: `the app brings its own runtime`).
The cell, its UI and its tests stay as they are; what the runtime trusts
is then yours to check.

## Channels and checks

- The channel followed is, most specific first: the launch flag
  `--channel=<c>`, then `AIORT_UPDATE_CHANNEL` (this run), the user's choice (`set_channel` on the `updates` cell,
  kept across runs), the channel the build was released for (a beta
  build never updates itself into stable), then `[updates] channel`.
- A running installed app first looks 30 to 90 s after start (spread, so
  many machines don't ask at once), then every `every` (or its
  `every_by_channel`). After failed checks it waits longer, doubling up
  to 4 × `every` (at least an hour, never past a day); a good check
  resets it. The `updates` health part is degraded after 5 failures in a
  row.
- Prereleases: `1.5.0-beta.2` is below `1.5.0`; digit runs compare as
  numbers (`alpha9 < alpha62`); `+build` is ignored. One not followed is
  said (`1.5.0-beta.2 is a prerelease, not offered here ([updates]
  prerelease = true follows them)`).
- A release older than or equal to the running version, one the person
  dismissed, and one that failed here before are never offered; a newer
  one is.
- The boot report says it in one line: `updates stable · manifest
  https://example.org/notes · every 6h · auto (aio.toml)`, or `updates
  not configured`.

### Stepping stones

`ax release notes --min-from 1.4.10` makes a release that installs only
over 1.4.10 or newer: for a step an older version can't make in one go
(a data format it can't read yet, a key it doesn't know yet). Keep
1.4.10 published on the channel. An older install finds the release
`blocked`, with one reason: `2.0.0 installs only over 1.4.10 or newer,
and this is 1.3.2: install 1.4.10 first, then update again`. Nothing gets
past it, not `apply_anyway` nor `start_fresh`: the download refuses it
too. `minFrom` is signed, so a host can't strip it, and a `minFrom` that
isn't a version refuses the release. `--min-from` above the release's
own version is refused by `ax release`.

## Delta updates

`--patch-from` makes patches, file by file, that turn each named release
into this one, listed and signed apart, with the release's own key, for
that very release. An app that runs one of them downloads the patch
instead of the whole bundle (`made from 1.4.11 and a patch`), checks
every file it made against the signed list, and falls back to the whole
bundle when anything doesn't match (`no patch used (…): downloading the
whole release`). A patch over half the release's size isn't used. An
AppImage's patch is one file, the image made from the image here (its
staged copy, else the AppImage it runs from), listed as
`<os>-<arch>.appimage.patches.json`: `ax release notes --appimage
--patch-from 1.4.11`. Keep the bundles (or images) of the releases you
patch from in `dist/<channel>/`.

## The data gate

A release says what data it can read: the store's layout and each cell's
data version (`ax release` asks the build, or `--data file`). The app
compares it with the data here before downloading, and again at the start
that would install it:

- A cell the release reads at an equal or newer version: fine (an older
  cell version always migrates, [persistence](../persistence/README.md)).
- A cell whose data here is newer than the release reads, or a newer
  store layout: **blocked**. The release is shown with its reasons
  (`notes: the data is v4, the release reads up to v3`) and never
  installed by itself.
- A cell the release no longer has: not a blocker; its data stays in the
  store.

Past a block the person (or the app) chooses:

| Method | The data | The way back |
|---|---|---|
| `apply_anyway` | kept, backed up first; the new version tries to read it | the backup, if it fails to start |
| `start_fresh` | moved aside whole to `archive/<id>-<version>-<time>` beside the data, never deleted; the new version starts on none | the data from before comes back whole, if it fails to start |

Starting fresh keeps the lock, the release keys, the `updates` cell, the
logs and the caches. A blocked release on a fresh install (no data yet)
is not blocked: there is nothing to read.

## Install, and the way back

1. A signed, newer, readable release is downloaded, checked and staged in
   `aio-rt/install/<id>/versions/<version>/` (`auto = true`). It needs
   about 3 × its size free there, said before a byte comes (`the update
   needs about 90 MB free in …`).
2. At the next start the data is backed up to
   `data/backups/pre-<version>-<time>.db` (the newest 3 kept), then a
   helper outside the app switches the installed version and starts the
   new one with the same arguments.
3. The new version must open its window and claim its first boot within
   120 s (a server or tool: within its start). One that exits at once is
   started once more.
4. If it doesn't, the helper stops it and starts the old version again,
   which puts the backup back (what the new one wrote is kept aside),
   never offers that release again, and files an `update:failed` report.
   Some version always runs.

On macOS, Windows and from an AppImage the helper is the new version's
own binary (the old one is in the place being replaced). The old app
exits only once the helper said it is up; a new binary that dies as it
starts (a crash before `aio_shell::start`, a missing library) is not
installed: the old version runs on, that release is never offered again
(`update failed: 1.5.0 could not be installed (…), so 1.4.11 runs on`).

When the first boot is claimed the install is final: `installed.json`
names the version, and versions past `keep` are removed (never the
running one or the one before it). A swap that couldn't be made (a file
held open) is tried again at the next start, three times at most.

Where each OS keeps it:

| OS | The installed app | An update |
|---|---|---|
| Linux (bundle) | `aio-rt/install/<id>/versions/<v>/`, `current` the running one, `~/.local/bin/<id>` through it | `current` switched |
| Linux (AppImage) | the AppImage file itself (updates kept in `install/<id>-appimage`) | that file replaced |
| macOS | the `.app` wherever it is | the `.app` replaced in place (one in a folder it can't write is said before downloading) |
| Windows | `%LOCALAPPDATA%\Programs\<id>` | the `.exe` (and a bundled runtime's folder) replaced |

An update that needs another Electron than the machine has fetches it
before it is staged.

### What the app says

```text
[aio] updates: 1.5.0 is ready: it installs the next time the app starts
[aio] data backed up before the update: …/data/backups/pre-1.5.0-2026-11-01T09-00-00-000Z.db
[aio] installing 1.5.0 (from 1.4.11)
[aio] first start of 1.5.0 after an update: it is kept once the window opens
[aio] update to 1.5.0 is in place
```

and when it goes wrong: `[aio] update failed: 1.5.0 did not start, so
1.4.11 runs again; the data from before it is back`. The helper's own
lines are in `logs/update.log`. `ax update <app>` shows the versions
installed and the update state.

aio-rs proves this path on every pull request, on Linux, macOS and
Windows: `scripts/update-proof` installs a sample service from its bundle,
updates it through a signed patch, then releases a build that dies as it
starts and checks that the version before comes back with its data
([ci](ci.md#aio-rss-own-proof-scripts)).

## Services and tools

A server or command-line app run by a supervisor (the systemd unit,
launchd agent or Windows task `ax build` writes beside a `cli` bundle,
which sets `AIO_SUPERVISED=1`) updates with nobody watching:

- With `auto = true` a staged release is installed at once (`installing
  1.5.0 now (auto, no window)`); with `auto = false` on a terminal it asks
  `Update to 1.5.0? The app will restart. [y/N]`, and with no terminal it
  only logs the offer.
- It never starts a process of its own: it exits, and the supervisor's
  restart is the new version. A new version that stops before it boots
  is started once more, then the old one comes back.
- Exit codes: an update or `aio::lifecycle::ask_restart()` exits **0**,
  so the supervisor starts it again; `ax stop` (or SIGTERM) under the
  supervisor exits **143** (`aio::lifecycle::STAYS_DOWN`), which the
  supervisor files take as "stay down" (`RestartPreventExitStatus=143`,
  with `AIO_STOP_STAYS_DOWN=1`).

From inside the app: `aio::lifecycle::ask_stop()` stops it the graceful
way (as SIGTERM), `ask_restart()` stops it to be started again, and a
server's `main` returns `aio::lifecycle::exit_code()`.

A desktop app with no supervisor restarts itself after `apply`: the app
stops, a detached copy waits for it to be gone, and that start installs
the release.

## The app's own update UI

The `updates` cell (`aio_shell::update::cell()`, an
`aio::update::Updates`; aio-rs-shell's `updates` feature, which
turns on aio-rs's) is what a UI shows and calls. With `auto = false`
a release is only offered, and installs when the app calls `apply`. The
cell's I/O is a runtime; tests use a stub:

```rust
use aio::update::{Found, Offer, Phase, Ready, Stub, Updates, UpdatesCalls, can_apply, install};
use std::sync::Arc;

static RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

# #[tokio::main]
# async fn main() {
let found = Found::Offer(Offer { version: "1.5.0".into(), notes: "Faster lists".into(), ..Offer::default() });
install(Arc::new(Stub(found))); // in the app: aio_shell installs the real one
let h = aio::Handle::<Updates>::memory();
h.ready(Ready { channel: "stable".into(), current: "1.4.0".into(), ..Ready::default() }).unwrap();

h.check().unwrap();
h.done("check").await;
let s = h.state();
assert_eq!(s.phase, Phase::Available);
assert_eq!(s.available.as_ref().map(|o| o.notes.as_str()), Some("Faster lists"));

// The app's say before any install, awaited: "not now" while a recording runs.
can_apply(|| async {
    match RECORDING.load(std::sync::atomic::Ordering::SeqCst) {
        true => Err("a recording is running".into()),
        false => Ok(()),
    }
});
RECORDING.store(true, std::sync::atomic::Ordering::SeqCst);
h.apply().unwrap();
h.done("apply").await;
assert_eq!(h.state().error.as_deref(), Some("the app says not now: a recording is running"));
assert_eq!(h.state().phase, Phase::Available, "still offered");
RECORDING.store(false, std::sync::atomic::Ordering::SeqCst);
h.apply().unwrap();
h.done("apply").await;
assert_eq!(h.state().phase, Phase::Restarting, "the stub steps through; the real one restarts the app");

h.dismiss().unwrap(); // "not this one": never offered again, a newer one is
assert_eq!(h.state().dismissed.as_deref(), Some("1.5.0"));
# }
```

| Method | Does |
|---|---|
| `check` | looks now (a check running is joined); refused while installing |
| `apply` | installs the offer now: staged if it isn't yet, the app restarts with the same arguments |
| `apply_anyway`, `start_fresh` | past the data gate: keep the data and try, or start on none |
| `dismiss`, `undismiss` | not this release (a newer one is offered) |
| `set_channel(c)` | follow another channel from now on (kept) |

The cell syncs to the app's clients too (a web page, a thin client, a
synced copy: `aio::sync::Remote<Updates>`), served beside the app's own
cells. A client sees all of it but `backup` and `retired` (paths on the
server's disk), and a path in `error` or `failed` is said as `…`; any client may `check`, `dismiss` and `undismiss`;
`apply`, `apply_anyway`, `start_fresh` and `set_channel` are an admin's
(`role = "admin"`: restarting the app is not a visitor's verb). An app
with its own runtime shares its cell with `aio::update::share(handle)`.

| State | Is |
|---|---|
| `on` | updates are set up (`[updates] url` and an installed copy) |
| `phase` | `Idle`, `Checking`, `Available`, `Blocked`, `Downloading`, `Installing`, `Restarting`, `Error` |
| `channel`, `current` | the channel followed, the running version |
| `available` | an `Offer`: `version`, `reason` (why it is offered, in the user's words), `notes`, `size` (0 once staged), `released_at`, `key`, `staged`, `migrates` (it moves a cell's data to a newer version; a backup is taken first) |
| `blocked` | a `Blocked`: `version` and `blockers`, the data gate's reasons |
| `progress` | percent of the download |
| `progress_of` | which download `progress` measures: `App`, then `Runtime` (the Electron the new version needs, once per machine: `progress` counts from 0 again); `None` when none runs |
| `checked_at` | when the last check answered (ms) |
| `error` | the last failure, verbatim |
| `failed` | an install that failed: (version, why), said again while it is offered |
| `backup` | the data's backup the update to this version took (a fresh start's: the store it set aside) |
| `retired` | where a fresh start to this version set the data aside (`archive/<id>-<version>-<time>`), for the app to say on its first start (aio's `freshStartNotice`) |
| `dismissed`, `pinned` | the release said no to, the channel chosen: the only saved fields |

- `aio::update::can_apply(veto)` (aio's `canApply`): an async closure
  awaited before every install: the cell's `apply`, `apply_anyway` and
  `start_fresh`, a server's unattended install, and a staged release's
  install at the next start (set it in `main`, before `aio_shell::start`).
  It runs off the cell, so it may do work first (a backup, a sync to
  finish). `Err(why)` is "not now": the release stays offered, `error`
  says `the app says not now: …`; at a start it stays staged for a later
  one (`update 1.5.0 waits: the app says not now: …` in the log). A start
  that installs with no veto set says so in the log, as aio warns. A veto
  that panics says not now.
- Only `dismissed` and `pinned` are saved: the rest is learned again
  each run, so a stale "available" never survives a restart.
- A release that failed its first start here comes back in `ready` as
  `rolled_back` and is dismissed.

### Testing the update banner

In the app's UI tests, `aio_shell::update::stub(current, found)` makes
`cell()` a fresh stubbed cell ready as `current`, whose checks find
`found` (`Found::Offer`, `Found::Blocked`, `Found::Current { reason }`)
and whose install steps the progress to 100 % and ends in `Restarting`,
touching nothing. Drive the banner on it as a person would, and wait on
its effects with the returned handle's `done("check")`.

## Android

An app can't replace its own APK: when a newer Android release
(`ax release --android`) is found, the `updates` cell shows it as
`blocked` with one reason, the link to open on the device. The person
taps it and Android's installer updates the app, which keeps its data
only when the APK is signed with the same key
([signing for the OS](#signing-for-the-os)).

## GitHub workflow

`ax release <app> --workflow` writes `.github/workflows/release-<app>.yml`
(never over one there: it is yours to edit):

- On a `v*` tag (or by hand), Linux, Windows (cross-built on Linux) and
  macOS each check the app out in `app/`, install `ax`, build and sign
  the release with `ax release`, and check that the tag is the app's
  version.
- Where `ax` and aio-rs come from: an app that depends on aio-rs by git
  installs `ax` from the commit its `Cargo.lock` pins. One `ax create`
  made (a path to a local checkout, which a runner doesn't have) gets
  aio-rs checked out beside it in `aio-rs/`, at the release tag of the
  `ax` that wrote the workflow (`v0.4.0`; the repository variables
  `AIO_REPO` and `AIO_REF` name another), its `Cargo.toml` paths pointed
  there, and `ax` installed from it. Nothing to change in the app.
- One job then puts every file on the tag's GitHub release, files before
  manifests, as a draft until all are there; on `stable`, it is marked
  latest only when it is the highest version tagged.
- Put the text of the key file in the repository secret
  `AIO_SIGNING_KEY`; without it the workflow fails, since an app refuses
  an unsigned release. Pair it with `url = "github:<owner>/<repo>"`.

### GitHub Pages

`ax release <app> --workflow --pages` (aio's `ship github`) also deploys
the channel to the repository's GitHub Pages site, after the files are
on the release: `<channel>/<os>-<arch>.json` and its bundles, so an app
reads them as from any web host:

```toml
[updates]
url = "https://<owner>.github.io/<repo>"   # the channel under it

[capabilities]
network = ["<owner>.github.io"]
```

- Once in the repository: Settings > Pages > Source: GitHub Actions; and
  let a tag deploy: Settings > Environments > github-pages, a tag rule
  `v*` (by default only the default branch may).
- `--web` adds a job that builds the app for the web (`ax build <app>
  --web`) and puts it at the site's root: the app's page at
  `https://<owner>.github.io/<repo>/`, its updates beside it.
- A deploy replaces the whole site: one channel per site. Pages takes
  files up to 100 MB; a bigger bundle stays on the release
  (`github:<owner>/<repo>`).

## Signing for the OS

- **Android**: `[build.android] keystore` and `key_alias`, or
  `AIO_ANDROID_KEYSTORE`, `AIO_ANDROID_KEY_ALIAS`, with the passwords in
  `AIO_ANDROID_KEYSTORE_PASSWORD` and `AIO_ANDROID_KEY_PASSWORD` (never in
  `aio.toml`). `ax build --android` signs with `apksigner`. Without a
  keystore it signs with a key made on first use in
  `~/.config/aio-rt/keys/<id>.android.p12`: keep it (Android updates an app
  only from the same key), but Play and other machines' builds won't match it.
- **Windows and macOS**: an app's own bundles are not OS-signed yet, so
  SmartScreen and Gatekeeper warn on first open. The update signature
  above is separate and always checked.

## Developer paths

- `ax update <app>`: an installed app's versions and update state.
- `ax update <app> --from-git <ref>`: builds that branch or tag of its git
  source (or of its checkout) and stages it for the next start,
  **unsigned** (never what the update checks do).

## Security summary

| Threat | What stops it |
|---|---|
| a download host or mirror serving its own build | the release must be signed by a key built into the app |
| a release's notes, version or data contract changed after signing | every field is signed |
| a release for another app, channel, OS or CPU | checked before the signature |
| a manifest naming `../` or another path | version and file names are plain names |
| a corrupted or swapped bundle or patch | size and SHA-256 from the signed manifest; each patched file against the signed list |
| a leaked key | rotate: the next release's roster drops it |
| a release that breaks on this machine's data | the data gate; a backup first; the old version back on a failed first start |
| a release older than the one installed | only newer versions are offered |
| a release an old install can't step to in one go | `minFrom`, signed: it is blocked there, with the version to install first |

What it doesn't do: OS code signing and notarization (Windows, macOS),
revocation of a key without a release, or checks on a custom `Runtime`'s
source.

## Common mistakes

- Releasing without `[updates] keys` in the build: no installed copy will
  trust the release.
- Losing the key file without a rotation: back it up, rotate early.
- Uploading manifests before bundles: an app may see a release it can't
  download yet.
- Releasing from an older branch: its build reads older data versions
  than installed copies have written, so they are blocked from it.
- Testing updates on `ax dev` or `cargo run`: only an installed copy
  updates (`ax install <app>` installs one).
- A tag that isn't the app's version: the workflow refuses it; locally,
  `[app] version` and the commit count make the number.

## Not in aio-rs

- OS code signing for desktop bundles (aio-rs 0.1.0).

## API

`cargo doc -p aio-rs --no-deps --all-features --open`: `aio::update`
(`Updates`, `Runtime`, `Stub`, `Manifest`, `Release`, `Roster`,
`Contract`, `check`, `check_served`, `blockers`, `migrates`, `below_min_from`,
`can_apply`, `installed`), `aio::lifecycle`
(`ask_stop`, `ask_restart`, `exit_code`, `STAYS_DOWN`); `cargo doc -p
aio-rs-shell --no-deps --open`: `aio_shell::update`.

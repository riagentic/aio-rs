# Versions and stability

Two kinds of version live here: aio-rs's own (the crates and `ax`), and
your app's (what `ax build` stamps into each build). This page says what
each promises and how each is made.

## aio-rs's version

aio-rs is `0.x`. The crates (`aio-rs`, `aio-rs-shell`, `aio-rs-macros`,
`aio-rs-config`, `aio-rs-config-macro`, `aio-rs-air`, `aio-rs-air-macro`,
`aio-rs-hotpatch`, `aio-rs-cli`) share one version,
the workspace's, and are released together; `ax version` and
`aio::VERSION` say it.

It follows [Semantic Versioning](https://semver.org/) with the 0.x rule:

| Change | Before 1.0 | From 1.0 |
|---|---|---|
| a public item removed or renamed, a signature changed | minor (`0.1` → `0.2`) | major |
| a default changed so an app behaves differently | minor | major |
| a new item, a new optional key, a new flag | patch or minor | minor |
| a fix that makes the code do what the docs say | patch | patch |

Every change is in [CHANGELOG.md](../../CHANGELOG.md), with what to do
when a minor version breaks something.

### What is public

- **The API**: what `cargo doc` shows for the crates
  ([API map](../reference/api.md)). Items marked `#[doc(hidden)]` (the
  macros' helpers, such as `aio::__config_lit`) and private modules are
  not part of it. `./ci.sh` checks it against the snapshot in
  `scripts/api/` (`scripts/public-api --check`), so every change to it
  shows in review.
- **`aio.toml`**: every section and key ([reference](../reference/aio-toml.md)).
  The file is checked as the app builds, so a removed key is a compile
  error that names it, never a silent change.
- **`ax`**: its verbs and flags ([ax](../clients/ax.md)).
- **The data on disk**: the store's tables, stamped as SQLite's
  `user_version` (layout 2 today; in an aio `state.db` that stays the
  app's, and aio-rs's tables say the layout). A newer aio-rs opens an older file and
  moves it on; an older one refuses a newer file instead of guessing.
  A cell's own data has its own `version` ([persistence](../persistence/README.md)).
- **The wire**: aio-rs's protocol on `/ws` (`x-aio-wire: 2`), and aio's
  protocol v3 for aio pages and tools with feature `compat`
  ([wire](../migration/wire.md)).

Not public: log and error wording (the words may change; a refusal's
meaning does not), the layout of `crates/`, anything only a test uses.

### Deprecation

An item on its way out stays and works for the rest of its minor line at
least, marked `#[deprecated]` with the replacement in its note, so the
compiler names every use. Where the change is mechanical, `ax lint --fix`
does it. An example: `aio::config::build`, the older launcher's
`build.rs` step, still builds, warns, and `ax lint --fix` deletes it.

### Gaps against aio's policy

- No `experimental` marker: every public item carries the same 0.x
  promise, and the API snapshot gate (`scripts/public-api --check` in
  CI) fails on any change, additive or not, so a marker would classify
  nothing. aio's marker only turned a removal into an additive change
  for its gate, and aio's own policy has no experimental items left.
- No release phases or freeze: aio froze its surface before its 1.0; aio-rs
  is early alpha, and a minor version may break things.

## Your app's version

An app's version is `major.minor.build`. You write `major.minor` in
`aio.toml`; `ax` numbers the build from git:

```toml
[app]
id = "notes"
version = "1.2"
```

| The tree is | The build is | Made from |
|---|---|---|
| a clean commit | `1.2.345` | `git rev-list --count HEAD` |
| uncommitted edits | `1.2.345-dirty.4e1d0c77` | the count, plus a hash of the edited files and their contents |
| not in git | `1.2.0-nogit.aabbccdd` | build 0, plus a hash of the app's files (`target/`, `dist/`, `node_modules/` and dot folders left out) |
| pinned: `version = "1.0.0"` | `1.0.0` | used as written, whatever the tree |
| staged: `version = "1.2-beta"` | `1.2.345-beta` | the count, then the stage |
| no `[app] version` | the package's `version` in `Cargo.toml` | used as written |

Two builds of one commit get the same version; every new commit raises
it. `ax` works the version out once per run, so what a build writes
(`Cargo.lock`, `dist/`) never renames a later target of the same run.

`-dirty` and `-nogit` are prereleases: they rank below the clean build of
the same count (`1.2.345-dirty.… < 1.2.345`), so an update check never
offers one over a release, and `ax release` refuses them
(`--allow-dirty` releases one anyway).

### Stages

`-alpha`, `-beta` or `-rc`, lower case, with no number after it: the
build count already numbers builds.

| Order | Why |
|---|---|
| `1.2.345-alpha` < `-beta` < `-rc` < `1.2.345` | prerelease order |
| `1.2.345-rc` < `1.2.346-alpha` | the build number first |
| `1.2.345-beta.dirty.4e1d0c77` < `1.2.345-rc` | one prerelease tail: the stage, then the mark |

Updates and stages: `[updates] prerelease` decides whether prereleases are
offered. Unset, an app follows them only when it is on one already or its
channel is not `stable`; so a `1.2.345-beta` is offered `1.2.346-beta`,
and a stable install never gets a beta ([release](../build/release.md#channels-and-checks)).

### Refused

Anything but `M.m` or `M.m.p`, with or without a stage, is refused by
name when `ax` builds: `"1"`, `"v1.2"`, `"1.x"`, `"1.2.3.4"`,
`"1.2-beta2"`, `"1.2-Beta"`, `"1.2-dirty"`. The message spells out both
forms.

### Where the version shows

`ax build` gives the binary its version as `AIORT_APP_VERSION`, read by
`aio::app_version!()` (the launcher passes it to `aio_shell::start`), so
every reader says the same string:

- `<app> --version` prints `<id> <version>` (the update check asks a
  bundle's binary this way);
- the health answer (`ax health`), problem reports and the app's About;
- `dist/manifest.json`: `app`, `version`, `commit`, `dirty`, each target's
  files;
- the bundle names: `<id>-<version>-<os>-<arch>.tar.gz` (Linux), `.zip`
  (Windows, macOS), `.dmg`, `.AppImage`; Android
  `<id>-<version>-android-<abi>.apk`; a delta update
  `<id>-<version>-from-<old>.patch`.

The installed program never has the version in its name: the Linux
installer puts it in `~/.local/share/aio-rt/install/<id>/versions/<version>/<id>`
and links `current` to it, so the data's home (from `[app] id`) is the
same across versions, and the version before stays for a rollback.

### Bumping, and history rewrites

Edit `[app] version` and commit. The count is the repository's, not the
minor's, so `1.3.346` follows `1.2.345`.

Squashing or rewriting history lowers the count: `0.1.52` can become
`0.1.5`. Android refuses that as an update (a lower `versionCode`; only an
uninstall gets past, and it deletes the app's data) and no update check
offers it. `ax build` compares with the last build's `dist/manifest.json`
and warns. Bump the minor first (`0.2.5` > `0.1.52`).

### Android

`versionCode` is `major × 1 000 000 + minor × 1 000 + build`, the stage
dropped: `1.2.3` is `1002003`, and `1.2.3-rc` has the same code, so
promote an rc on a later commit. Limits: minor and build at most 999,
major at most 2000; past that the Android build is refused with the
reason. The build is the repository's commit count, so a repository past 999
commits can't build for Android with a derived version: pin `M.m.p`.

## Common mistakes

- Putting the version in the program's file name, or changing `[app] id`
  per version: the id is where the data lives, so the app starts empty.
- Releasing from uncommitted edits: refused; commit first.
- `"1.2-rc1"`: refused; the build count is the number.
- Squashing history on an app already shipped: the next build is lower.
  Bump the minor in the same change.

# aio.toml reference

`aio.toml` is an app's one config file, beside its `Cargo.toml`. This
page is every section and key: its type, default, the values it takes,
what it does, and what goes wrong. The last part maps every option of
aio's `cell(...)` and `aio.run(...)` to where it lives in aio-rs.

1. [How the file is read](#how-the-file-is-read)
2. [Value types](#value-types)
3. [A whole file](#a-whole-file)
4. Sections: [app](#app), [data](#data), [home](#home),
   [history](#history), [server](#server), [client](#client),
   [accounts](#accounts), [capabilities](#capabilities),
   [updates](#updates), [build](#build), [log](#log), [perf](#perf),
   [memory](#memory), [budgets](#budgets), [feedback](#feedback),
   [ui](#ui), [vitals](#vitals)
5. [Options outside aio.toml](#options-outside-aiotoml): aio's cell and
   `aio.run` options, and where each one is in aio-rs
6. [Common mistakes](#common-mistakes)

## How the file is read

`aio::config!()` parses the file as the app *builds*:

- It takes the nearest `aio.toml` from the crate's folder up, so the
  launcher crate (`app/`) finds the app's file one folder up.
  `aio::config!("../aio.toml")` names a file instead, relative to the
  crate's folder.
- Every key has a default, so a file holding only `[app] id` is a whole
  config.
- An unknown key, a wrong type or a value the checks refuse is a
  **compile error** that names the key (`aio.toml: [ui] hue is degrees,
  0 to 359`, `unknown field 'nmae'`). A typo never silently does nothing.
- The binary holds the result as Rust values, not a TOML parser: nothing
  is read at run time, and editing the file means building again (cargo
  rebuilds on its own when the file changes). A splash image is built in
  too.
- What varies per launch is a flag or a variable, never this file:
  `--home`, `--profile`, `--instance`, `--takeover`, `--port`, `--host`,
  `--expose` ([project](../basics/project.md#the-launcher)), and the
  variables in [environment](environment.md).

`ax config <app>` prints the file as the build bakes it in, defaults
included. `ax doctor <app>` says who decided each setting of a running app
(the file, a flag or a variable). The types are in `aio::config`
(`cargo doc -p aio-rs --no-deps`, `aio::config::Config` and one struct
per section).

## Value types

| Type | Written as | Rules |
|---|---|---|
| span | `"90d"`, `"6h"`, `"30m"`, `"15s"` | a whole number, then one unit: `d`, `h`, `m` or `s`. No fractions (`"1.5h"`), no mixes (`"1h30m"`). |
| span or off | a span, or `"off"` | `"off"` turns the thing off (or "no limit": each key says). |
| size | `"1MB"`, `"512KB"`, `"1.5 MB"`, `2048` | a number (fractions allowed), then `B`, `K`/`KB`, `M`/`MB` or `G`/`GB`, any case; `KB` = 1024. A bare number is bytes. Zero is refused. |
| rate | `"20/s"`, `"20/sec"`, `"20 per second"`, `20` | per second; zero is refused. |
| milliseconds | `5`, `2000` | a whole number. |
| id | `"notes"` | letters, digits, `-`, `_`, `.`; not empty, not starting with `.`. |
| host | `"api.example.org"`, `"10.0.0.5:8443"`, `"*.example.org"` | no scheme, path, `@` or space. |

## A whole file

Every section, most keys at their default (each table below says the
rest).

```toml
[app]
id = "notes"
name = "Notes"
mode = "standalone"
version = "1.4"
ui = "window"
lang = "en"
dir = "ltr"
tz = "utc"
profiles = true
maintainer = "Example Team <team@example.org>"
# port_of = "notes"         # a port of the aio app "notes": its home and state.db

[data]
durability = "normal"
store = "state.db"
retire = []
pragmas = []
persist = true
only_persist = false
coalesce_ms = 100
backup_every = "off"
backup_keep = 7

# [home]
# dir = "/srv/notes"        # the home itself (default: aio-rs's place)

[history]
keep = "90d"

[server]
discover = false
pairing = false
tls = false
hosts = []
strict_origin = false
open = false

[server.security]
headers = true
csp = "basic"
frame_options = true
referrer = "no-referrer"
hsts = true
nonce = false

[server.limits]
clients = 256
max_message = 4194304
calls_per_sec = 200
bytes_per_sec = 8388608
sync_interval_ms = 50
full_state_percent = 50

[client]
retention = "off"
strict_pin = false
transport = "auto"

[accounts]
on = false
signup = false
session = "30d"
max_sessions = 10
require_verified = false
totp = true
cookie = true

[capabilities]
network = []

[updates]
keys = []
channel = "stable"
every = "6h"
auto = true
keep = 3

[build]
targets = []
smoke = false
platforms = []
runtime = "shared"
chromium_extras = "keep"

[build.android]
aab = false
schemes = []
camera = false
color_scheme = "system"

[log]
level = "info"
keep = 5
file_mb = 10
heartbeat = 3600
suppress = []
per_run = false
# dir = "/var/log/notes"    # default: <home>/logs

[perf]
effect = 5
call_ceiling = 30000

[perf.storm]
rate = 200
secs = 5
breaker = false

[memory]
high = 50
clear = 40
critical = 80
growth_mb = 256
every = "10s"

[budgets]

[feedback]
timeout = "15s"
keep = 50
auto = true
args = false

[ui]
theme = "auto"
scheme = "system"
neutral = "tinted"
layout = true

[vitals]
hints = true

[vitals.render]
degraded = 50
warning = 200
frozen = 2000
```

## [app]

Who the app is and where its cells live.

| Key | Type | Default | What it is |
|---|---|---|---|
| `id` | id | required | The app's identity: its data's home, its install, its sockets. Never change it once the app has run: the data is found by it. |
| `name` | string | `""` | The name people see: the window title (`aio_shell::Window::of`), the menu entry, `{title}` in the splash. |
| `mode` | `standalone`, `client-server`, `server`, `client`, `cli` | `"standalone"` | Where the cells live. `standalone`: in the app, nothing listens. `client-server`: in the app, served to clients, with a window. `server`: served, no window. `client`: a window on another process's cells (`[client] server`, or a pairing). `cli`: a command-line tool. See [concepts](../basics/concepts.md#where-code-runs). |
| `version` | string | the package's | `major.minor`: builds are numbered from git, `1.4.<commit count>` (`1.4.0-nogit` without git). Or a pinned `major.minor.patch`. Either may carry a stage: `1.4-beta` builds `1.4.<n>-beta` (`-alpha` < `-beta` < `-rc` < none). Unset: `Cargo.toml`'s version, as written. |
| `ui` | `window`, `browser` | `"window"` | Where a `client-server` app shows its UI: `window` (the shared runtime's) or `browser` (its own page in the system browser, running until asked to stop). A launch's `--client=window`, `browser` or `server-only` (no window, served until asked to stop) decides for one run (aio's `--client`). |
| `lang` | language tag | `"en"` | The page's language, `<html lang>` (screen readers, spellcheck, translation). Letters, digits and `-`: `"en"`, `"pt-BR"`. |
| `dir` | `ltr`, `rtl`, `auto` | `"ltr"` | The page's direction, `<html dir>`. `auto` is the browser's guess from the first strong character. Never guessed from `lang`. |
| `tz` | `utc`, `local` | `"utc"` | The clock cron schedules read (`#[cron]`, `cx.cron`): UTC as aio's, so a port's jobs fire when they did, or this machine's local time (with its daylight-saving changes). |
| `flags` | list | `[]` | the app's own launch flags, `--name` each (aio's `appFlags`). Declared, a launch flag (before a bare `--`) one typo from one of them or of aio's own (`--profile`, `--route`, ...) is refused at start: `--verbos: no such flag (did you mean --verbose?)`. A flag far from every name still reaches the app (`aio::paths::Ask::app_args`), so Electron's and the app's undeclared flags stay quiet. Empty: nothing is checked. |
| `profiles` | bool | `true` | `false` (aio's `profiles: false`: a kiosk, a service): the app runs from one home only, and a launch naming another (`--profile`, `AIORT_PROFILE`, `AIO_PROFILE`, `--home`) is refused at start. `--instance` and `AIORT_DATA` still give a sandbox. |
| `maintainer` | `"Name <email>"` | none | Who publishes it. A Debian package needs it (`ax build --deb`); `ax create` fills it from git. |
| `port_of` | id | none | The aio app this app is a port of (its `appId`): it runs on that app's home (its profiles, `AIO_APPS_DIR`) and `state.db`, which aio can still open after. See [migration: data](../migration/data.md). |

Checked as the app builds:

- `mode = "client-server"` and `mode = "server"` need `[server] listen`.
- `ui = "browser"` needs `mode = "client-server"` (its server serves the
  page).
- `mode = "client"` builds without `[client] server`: a pairing or the
  connect page can give it the server. With neither, a window's client
  shows its connect page; a CLI's refuses to start and says so.

```toml
# A server and its own window, shown in the system browser.
[app]
id = "shop"
name = "Shop"
mode = "client-server"
ui = "browser"
version = "2.3-rc"

[server]
listen = "127.0.0.1:7420"
```

## [data]

The store: one SQLite file in the app's `data/` folder
([persistence](../persistence/README.md)).

| Key | Type | Default | What it is |
|---|---|---|---|
| `durability` | `normal`, `full` | `"normal"` | How hard a commit is saved. `normal`: survives the app crashing; the last commits may be lost on power loss (WAL, `synchronous = NORMAL`). `full`: each commit is on disk before the call returns (`synchronous = FULL`). Phones always use `full`: the OS may kill the app at any moment. |
| `store` | file name | `"state.db"` | The store's file name in `data/` (aio's `dbPath`). |
| `retire` | file names | `[]` | Files in `data/` set aside at boot as `<name>.pre-migration` (with their `-wal` and `-shm`), never deleted: an old store the app no longer opens. |
| `pragmas` | statements | `[]` | Extra `PRAGMA` statements run on the store after aio's own (aio's `dbPragmas`). One SQLite ignores refuses the boot. |
| `persist` | bool | `true` | `false`: the state lives in memory only, no store is opened, every launch starts from the defaults (aio's `persist: false`; the flag `--no-persist` does it for one launch). |
| `only_persist` | bool | `false` | `true`: only `#[cell(persist)]` cells are saved; every other cell opens in memory and starts from its defaults each launch (aio's `cellDefaults: { persist: "none" }`). `false`: every cell opened on the store is saved, unless `#[cell(persist = false)]`. |
| `coalesce_ms` | ms | `100` | How long a run of `#[coalesce]` calls is held before it is written (aio's `persistDebounceMs`); `0` writes each call. At most 60000. |
| `backup_every` | span or `"off"` | `"off"` | Timed whole backups into `<home>/backups` while the app runs ([backups](../persistence/files.md#scheduled-backups)). |
| `snapshot_every` | span or `"off"` | `"off"` | How often the store's good copy is made again while the app runs (aio's rolling `db.snapshot` that `checkIntegrityOnBoot` restores from): a damaged store then loses at most this much. `"off"`: once a day, at boot. Each copy is checked before it replaces the last. |
| `backup_keep` | count | `7` | Timed backups kept, the newest (at least 1); hand-made ones are never removed. |

Checked: `store` and each `retire` entry are file names (no `/`, `\`, `.`
or `..`); `retire` can't name the store; each pragma is a `PRAGMA`
statement and sets none of `journal_mode`, `synchronous` (that is
`durability`) or `user_version`, which the store relies on.

Every open also runs `PRAGMA quick_check` and, on a damaged file, sets it
aside and puts the last good copy in its place (aio's
`checkIntegrityOnBoot`, always on). `snapshot_every` keeps that copy
fresh.

```toml
[data]
durability = "full"
pragmas = ["PRAGMA cache_size = -16000"]
retire = ["old-notes.db"]
```

## [home]

Where the app's home is, as its author fixes it. `--home`, `--profile`,
`--instance` and `AIORT_DATA` still win. `dir` is any app's; `env`,
`portable` and `dev` are a port's (`[app] port_of`): the aio app's own
rules, as its code resolved them, refused without `port_of`.

| Key | Type | Default | What it is |
|---|---|---|---|
| `dir` | absolute or `~/…` | none | The home itself (aio's `appDir`): `data/`, `logs/` and `cache/` in it, a profile beside it (`<dir>-<name>`). Over aio-rs's default place (a port's: after its own rules, over `AIO_APPS_DIR`, as aio's). The boot report says `[home] dir`. Any OS's absolute path passes the build (`/srv/notes`, `C:\notes`), as the app may be built for another; the app refuses one that isn't absolute where it runs. |
| `env` | variable name | none | A variable naming the home (`NOTES_HOME`); it beats `AIO_APPS_DIR`. |
| `portable` | file name | none | A folder beside the app's binary (`notes-data`): the home when it exists (a portable install). Not in a debug build. |
| `dev` | file name | none | A debug build's home in the user's folder (`.notes-dev`), so a dev run never opens the real data. A profile counts from the plain home. |

```toml
[app]
id = "notes"
port_of = "notes"

[home]
env = "NOTES_HOME"
dev = ".notes-dev"
```

```toml
[home]
dir = "/srv/notes"
```

## [history]

| Key | Type | Default | What it is |
|---|---|---|---|
| `keep` | span | `"90d"` | How long the event log keeps every step before older steps fold into a snapshot. Replay, `ax events` and time travel reach back this far. Folded at boot and every 256 commits. |
| `travel` | bool | `true` | Time travel may step through the history (aio's `diagnostics.dev.timeTravel`). `false` refuses every step (`ax timetravel`, the panel); the history is kept the same. |

A longer `keep` costs disk (one row per committed call); a shorter one
loses the steps `ax timetravel` and `ax replay` could show.

## [server]

For `mode = "client-server"` and `"server"` ([sync](../state/sync.md)).

| Key | Type | Default | What it is |
|---|---|---|---|
| `listen` | `host:port` | none | Where to serve (`"127.0.0.1:7420"`). Required by the two serving modes; a port in use stops the start. `--port`, `--host` and `--expose` change it per launch. |
| `discover` | bool | `false` | Answer LAN discovery probes (UDP 8099, aio's format), so `ax discover` and clients find it. |
| `pairing` | bool | `false` | Let new devices pair with a PIN (`ax pair`). |
| `tls` | bool | `false` | Serve over TLS with this machine's local root (`ax trust` explains how browsers trust it). |
| `cert`, `key` | absolute paths | none | With `tls`: the operator's own certificate chain and key (PEM) instead of the local root, e.g. one a public CA signed. A launch's `--tls-cert` and `--tls-key` override them (and turn TLS on); `--no-tls` turns it off. |
| `hosts` | strings | `[]` | Names it is reached as besides IPs, `localhost` and `.local` (a reverse proxy's, a domain); any other `Host` is refused. The same list says which origins may frame the app and open its socket (aio's `allowedOrigins`). |
| `strict_origin` | bool | `false` | Served beyond loopback: a socket upgrade must say its `Origin`. |
| `trust_proxy` | header name | none | Behind a reverse proxy: the header it overwrites with the client's address (`"x-forwarded-for"`); its last hop is then the client. |
| `open` | bool | `false` | Open on purpose (aio's `key: false`): links and routes need no key, so anyone who reaches the port reads the state and calls `#[client]` methods. Said loudly at boot and by `ax doctor`. |
| `page_log` | bool | `true` | A web page's console in the app's log as `[page]` lines, a release too (aio's client log): its pages post it to `/__aio/log` (with the key or a session, unless from this machine), at most 100 lines a second for each client (the rest counted and said). `false`: nothing is taken. |
| `local_peers` | bool | `true` | Other processes of this user reach a release build's cells through its local door, with a client's rights (`ax state`, `ax call`, other apps as peers: aio's `allowLocalPeers`, which aio leaves off). `false`: a release build opens no door; a debug build always does. |

Checked: `cert` and `key` go together, need `tls = true` and are absolute
on some OS (the file is built into the app: a relative path has no folder
to be relative to); `trust_proxy` is a header name; `open` and `[accounts] on`
(or `sessions`) are not both set (each says who gets in).

The framework's own paths are `/ws` and `/__aio…`; an app's routes and
dirs can't take them.

```toml
[server]
listen = "0.0.0.0:7420"
tls = true
cert = "/etc/notes/fullchain.pem"
key = "/etc/notes/privkey.pem"
hosts = ["notes.example.org"]
strict_origin = true
trust_proxy = "x-forwarded-for"
```

### [server.security]

The headers every answer carries, decided once at boot. Each default is on
only where it can't break a working app.

| Key | Type | Default | What it is |
|---|---|---|---|
| `headers` | bool | `true` | Send them (`false`: only `nosniff`). |
| `csp` | string | `"basic"` | `basic` (`base-uri`, `object-src`, `form-action`, `frame-ancestors` and a `script-src` without `'unsafe-eval'`; no `default-src`, so off-origin assets still load), `strict` (adds `default-src 'self'` and per-type sources), `off`, or any other string: a whole policy used verbatim. |
| `directives` | table | `{}` | Per directive: a string replaces or adds it (`"img-src" = "'self' https:"`), `false` drops it. `{nonce}` names the nonce. Not with a verbatim policy. |
| `frame_options` | bool | `true` | `X-Frame-Options: SAMEORIGIN` when `hosts` names nobody who may embed the app. |
| `referrer` | string | `"no-referrer"` | `Referrer-Policy`. |
| `hsts` | bool or string | `true` | `Strict-Transport-Security`, only behind the operator's own certificate (`cert`): `true` is 180 days, a string is used verbatim, `false` none. On the local root it never is: a pin would outlive the app. |
| `permissions` | string | none | `Permissions-Policy`, verbatim. None is guessed (camera, microphone: the app says). |
| `nonce` | bool | `false` | With `csp = "strict"`: a per-answer nonce on the shell's scripts instead of `'unsafe-inline'`. |

A value no header can carry is refused as the app builds, naming the key,
never a 500 on every answer.

```toml
[server.security]
csp = "strict"
nonce = true
permissions = "camera=(), microphone=()"

[server.security.directives]
"img-src" = "'self' https: data:"
"base-uri" = false
```

### [server.limits]

Each link's limits (aio's `wsLimits`, `maxConnections`). A refused client
gets 503 with the reason, a too-big message a close (1009) naming the
limit; a fast sender is read more slowly, so nothing is dropped. Clients
learn them on connect, check sizes before sending and pace themselves.

| Key | Type | Default | What it is |
|---|---|---|---|
| `clients` | number | `256` | Open links at most. |
| `max_message` | bytes | `4194304` (4 MB) | Largest message a client may send. |
| `calls_per_sec` | number | `200` | Calls a link may make per second (bursts up to twice that). |
| `bytes_per_sec` | bytes | `8388608` (8 MB) | Bytes a link may send per second (bursts up to twice that). |
| `sync_interval_ms` | ms | `50` | A link is sent at most one change per this window (aio's `syncIntervalMs`): the first of a burst at once, the rest folded into one patch; a person's call (a user's or an agent's) goes at once. `0`: each change at once. |
| `full_state_percent` | percent | `50` | A patch over this share of the whole view goes as the view (aio's `fullStateThreshold`, `0.5` = `50`). `0`: always the view. Patches under 256 bytes always go as patches. |

### [server.dirs], [server.public_dirs]

Extra read-only folders served by path prefix (aio's `assets` and
`publicDir`). `dirs` need the key (or a session); `public_dirs` are for
anyone: only what a page shown before sign-in needs. A relative folder is
in the app's files tier (`<data>/files`, never its key or store), an
absolute one the operator's.

```toml
[server.dirs]
"/media" = "media"

[server.public_dirs]
"/brand" = "/srv/notes/brand"
```

Checked: a prefix starts with `/`, is not `/`, `/ws` or `/__aio…`, has no
empty segment and no segment starting with `.` (`.well-known` aside); a
folder is not empty and has no `..`; no prefix is served twice.

## [client]

For `mode = "client"`, and the synced copy of a `client-server` app.

| Key | Type | Default | What it is |
|---|---|---|---|
| `server` | `host:port` | none | The app's server. A link the device keeps (`ax pair`, `ax import`, the connect page) wins over it, and the launch's `--server-url` over both. Reached without a `[capabilities] network` entry: naming it is the consent. |
| `retention` | span or off | `"off"` | How long a call made offline is kept once the queue is full: then a new call drops the oldest past it, and says so. `"off"`: nothing is dropped; a full queue refuses new calls. |
| `strict_pin` | bool | `false` | A paired server whose certificate its pinned root no longer covers is refused until paired again (aio's `strictPin`; `--strict-pin` for one launch). Unset, one that proves it holds the app's key is pinned again ([desktop](../clients/desktop.md#a-certificate-that-changed)). |
| `transport` | `"auto"`, `"ws"`, `"poll"` | `"auto"` | How the copy reaches its server (aio's `transport`; `--transport=` for one launch): a WebSocket, plain HTTP requests when one doesn't get through, or only one of them ([transports](../clients/transports.md#when-websockets-dont-get-through)). |

### [client.timeouts]

Per method, how long a confirmed call waits for the server's answer (else
15 s), or `"off"` (as long as it takes):

```toml
[client]
server = "notes.example.org:7420"
retention = "7d"

[client.timeouts]
"notes.import" = "60s"
"notes.export" = "off"
```

## [accounts]

Users, roles and sessions, in `data/auth.db` ([accounts](../auth/README.md)).

| Key | Type | Default | What it is |
|---|---|---|---|
| `on` | bool | `false` | The shared key no longer opens sync; a session does, and the caller is the session's user. |
| `sessions` | bool | `false` | Sessions without accounts (aio's `sessions: true`): no users, no login doors; the app issues sessions (`auth::serving()`'s `issue`), and a session token is that user on every door. `on` includes it. |
| `signup` | bool | `false` | Anyone may make an account. |
| `session` | span | `"30d"` | How long a session lives unused (used past half its life, it is renewed). 1 s to 100 years. |
| `max_sessions` | number | `10` | Live sessions per user; a new one past it ends the oldest. At least 1. |
| `require_verified` | bool | `false` | No sign-in until the account's email is verified (the app's mail hook, `Http::mail`, or `ax auth <app> verify <id>`). |
| `totp` | bool | `true` | Users may turn on a second factor, TOTP (aio's `auth.totp`). `false`: enrollment is refused (`totp_disabled`); a factor already on stays on. |
| `cookie` | bool | `true` | A sign-in sets the HttpOnly session cookie (aio's `auth.cookie`). `false`: the session is the answer's token alone, a bearer. |

`session` and `max_sessions` are checked only with `on` or `sessions`;
`signup` and `require_verified` need `on`. `signup` with
`require_verified` needs the server's mail hook (`Http::mail`): without
one no one who signs up could ever sign in, and the app refuses to serve,
saying so (aio's boot refusal). With `signup` off, the operator verifies
(`ax auth <app> verify <id>`).

```toml
[accounts]
on = true
signup = true
session = "14d"
max_sessions = 3
require_verified = true
```

## [capabilities]

| Key | Type | Default | What it is |
|---|---|---|---|
| `network` | hosts | `[]` | Hosts aio's outbound requests may reach besides this machine (loopback is always allowed). Everything else is refused. |

Each entry is one of: a host (`"api.example.org"`), a host and port
(`"10.0.0.5:8443"`: only that port), `"*.example.org"` (any name under it,
not the bare domain), `"lan"` (private addresses) or `"*"` (anything).

The hosts this file names must be in it, or the build fails and says
which line to add: `[feedback] url` and `[updates] url` (a `github:`
source needs `"github.com"` and `"*.githubusercontent.com"`; a
`git+file://` one needs nothing).

A client's server is the exception, as aio's thin client: the one the
person picks (the connect page, a pairing, `--server-url`) or the app
names (`[client] server`) is reached without an entry, that host and port
alone. Every other host still needs one.

```toml
[capabilities]
network = ["api.example.org", "*.githubusercontent.com", "github.com", "lan"]
```

## [updates]

Signed updates ([release and update](../build/release.md)). Off without
`url`. A `url` also turns on aio-rs-shell's `updates` in ax's builds:
an app without one carries no updater ([project](../basics/project.md)).

| Key | Type | Default | What it is |
|---|---|---|---|
| `url` | string | none | Where releases are: `https://…` (`<url>/<channel>/<os>-<arch>.json`), `github:<owner>/<repo>` (its GitHub releases), or a repository (`git+https://`, `git+ssh://`, `git+file://`) whose channel branch or tag is built on the machine when its commit or tag is signed by one of `keys`. `http://127.0.0.1:<port>` only for tests. |
| `keys` | strings | `[]` | The release keys the app trusts (`ed25519:<base64>`, printed by `ax keygen <app>`); a git source's may be OpenSSH lines (`ssh-ed25519 <base64>`). Required with `url`: no trust on first use. |
| `channel` | name | `"stable"` | The channel followed: letters, digits, `-` `_` `.`, at most 64, not starting with `.`. `AIORT_UPDATE_CHANNEL` changes it per launch. |
| `every` | span or off | `"6h"` | How often a running app looks, or `"off"` (only when the app asks: the `updates` cell's `check`). |
| `auto` | bool | `true` | `true`: downloaded in the background, installed at the next start (a server, with no one to ask: at once). `false`: only offered; installed when the app says (the cell's `apply`). |
| `prerelease` | bool | unset | Offer prereleases (`1.5.0-beta`). Unset: only off `stable`, or to an app on a prerelease already. |
| `keep` | number | `3` | Installed versions kept, the running one included. At least 2: a rollback needs the one before. |

### [updates.every_by_channel]

`every` for some channels: where you ship into it now, a check a minute;
where users are, a few a day.

```toml
[updates]
url = "github:example/notes"
keys = ["ed25519:<the line ax keygen printed>"]
channel = "stable"

[updates.every_by_channel]
dev = "1m"
beta = "15m"
```

## [build]

What `ax build <app>` makes ([build](../build/README.md),
[targets](../build/targets.md)).

| Key | Type | Default | What it is |
|---|---|---|---|
| `targets` | list | `[]` | Kinds, in order: `desktop` (this OS's bundle), `windows`, `deb`, `appimage`, `dmg`, `cli`, `android`, `web`; or `{ name, kind, bin }` for a labelled one (two of a kind, or another binary of the workspace, which has its own `aio.toml`), which may carry its own `platforms`, `title` and `ui` (over `[build] platforms`, `[app] name` and `ui`; `entry` is aio's spelling of `bin`). Empty: `desktop` (a `cli` or `server` app: `cli`). `ax build <app> --list` lists them, `--targets a,b` picks some. |
| `platforms` | list | `[]` | The OS and CPU each desktop target is built for: `linux`, `linux-arm64`, `windows`, `windows-arm64`, `macos`, `macos-arm64`, `host`. Empty: this machine. `--platforms a,b` per build. |
| `out` | folder | `"dist"` | Where `ax build` puts what it makes and `ax release`, `ax install` and `ax lab` look (aio's `build.out`): relative to the workspace, or absolute. `--out dir` wins for one run. |
| `budget_mb` | number | 10 (a server: 6) | The binary's size budget in MB; the build fails above it and names the crates that take the space (`ax build --sizes`). For apps with heavy libraries. |
| `css` | command or list | none | A CSS toolchain run before every build and on every change under `ax dev`: a command line split on spaces, or an argv list. Never through a shell. |
| `smoke` | bool | `false` | Every build then starts each Linux artifact it made headless, stops it and checks its log (as `--smoke`). |
| `runtime` | `shared`, `bundled` | `"shared"` | Where a desktop app's Electron comes from: `shared` (one per machine, fetched at the first start) or `bundled` (in the package beside the binary, trimmed: an offline install; a macOS one is made on a Mac). |
| `chromium_extras` | `keep`, `strip` | `"keep"` | A bundled runtime's optional GPU extras (software Vulkan, DirectX shader compilers): `strip` for a smaller package that draws less well where no GPU driver works. |

```toml
[build]
targets = ["desktop", "deb", { name = "pro", kind = "desktop", bin = "notes-pro" }]
platforms = ["linux", "windows"]
css = ["tailwindcss", "-i", "src/app.css", "-o", "assets/style.css"]
budget_mb = 14
```

### [build.assets]

Folders built into the app by URL prefix (aio's `assets`): each file is
answered at the same path in the window (`aio://app/media/…`), by the
app's server (to anyone: it ships with the app, as its page does), in the
web build (cached offline) and in the APK. A folder is beside `aio.toml`
(relative, no `..`), a prefix is a path like `/media` (not `/`, `/ws` or
`/__aio…`) and not also in `[server] dirs`; dotfiles are left out, and a
file name with `:` or `*` is refused. `ax build`, `ax dev` and `ax start`
build a file added later in; a plain `cargo build` doesn't see a new one.

```toml
[build.assets]
"/media" = "media"   # media/song.mp3 is /media/song.mp3
```

### [build.android]

| Key | Type | Default | What it is |
|---|---|---|---|
| `application_id` | package name | `aio.rt.<id>` | The app's permanent identity on Android and in Play: two or more dot-separated segments, each a letter then letters, digits or `_` (`com.example.notes`). |
| `keystore` | path | aio's own key | The release keystore (relative: to the app's folder). Its password is never here: the build reads `AIO_ANDROID_KEYSTORE_PASSWORD`; `AIO_ANDROID_KEYSTORE` overrides the path. |
| `key_alias` | string | none | The key in it (`AIO_ANDROID_KEY_ALIAS` overrides). |
| `aab` | bool | `false` | Also an Android App Bundle (`.aab`) for Play. |
| `schemes` | list | `[]` | URL schemes the app opens (`notes://item/7`): each link reaches the running app (`aio::ui::links`; a `router!` goes there). A lower-case letter, then lower-case letters, digits, `+`, `-` or `.`; none of the system's (`http`, `https`, `content`, `file`, `intent`, `mailto`, `tel`…). |
| `camera` | bool | `false` | The page may ask for the camera: the APK declares it and Android asks once. Off: refused. |
| `color_scheme` | `system`, `light`, `dark` | `"system"` | Light or dark whatever the phone's setting; `system`: the phone's. |

### [build.macos]

| Key | Type | Default | What it is |
|---|---|---|---|
| `bundle_id` | bundle id | `aio.rt.<id>` | The app's permanent macOS identity (`CFBundleIdentifier`): two or more dot-separated segments of letters, digits and `-` (`com.example.notes`). Used as written. |
| `host` | string | none | The Mac a build away from one runs on, over SSH (`[ssh options] [user@]host`); it outranks `AIO_MACOS_SSH`. |

### [build.windows]

| Key | Type | Default | What it is |
|---|---|---|---|
| `shortcut` | bool | `true` | The one-click download adds a Start menu entry on its first install (aio's `build.windows.shortcut`); `false`: none. |

### [build.download_window]

The window a first start shows while Electron downloads, and its Retry /
Quit question (aio's `electron.downloadWindow`). Every key is optional; the
defaults are aio's. The texts take `{title}` and `{version}` (Electron's).

| Key | Type | Default | What it is |
|---|---|---|---|
| `heading` | string | `"Getting {title} ready"` | Its heading. |
| `text` | string | `"Downloading Electron {version} — first start only · {got} / {total} MB"` | The line under the bar: also `{got}`, `{total}` (MB, `?` while unknown) and `{pct}`. |
| `question` | string | aio's, naming what failed and how to fix it | The Retry / Quit question; `{reason}` is what failed. |
| `background`, `color`, `bar`, `track` | `#rgb` / `#rrggbb` | the app's, light or dark as the OS is | Its background, text, bar fill and bar ground; one set is used in both modes. |
| `cancel` | bool | `true` | Its Cancel button; closing the window cancels either way. |

```toml
[build.download_window]
heading = "Setting up {title}"
bar = "#14f195"
```

### [build.splash]

The app's image (and text) on screen from its start until its window has
loaded, on the desktop, the web and Android (aio's `build.splash`). On the
desktop the OS draws it before Electron starts, at every start, and a first
start's Electron download runs on it, its bar under the text
([desktop](../clients/desktop.md#the-splash)).

| Key | Type | Default | What it is |
|---|---|---|---|
| `image` | path | required | A `.png` in the app's folder (`"assets/splash.png"`, relative, no `..`); `…@2x.png` is drawn at half its pixel size. It must exist as the app builds: it is built in. |
| `text` | string | none | Under the image, on the desktop, the web and Android: `{title}` is the app's name, `{version}` its version (what `--version` prints); `{got}`, `{total}` and `{pct}` read `0`, `?`, `?`; any other `{word}` stays as written. |
| `min_ms` | milliseconds | `400` | It stays at least this long once shown. |
| `max_ms` | milliseconds | `15000` | No window by then: it goes anyway. At least `min_ms`. |
| `background` | `["#light", "#dark"]` | the app's colours | Each `#rgb` or `#rrggbb`. |
| `show` | `always`, `first-start`, `never` | `"always"` | When it shows: every start; only while a first start downloads Electron; never (the download window is then the plain one). `Window { splash_show }` decides at run time instead. |
| `download_text` | string | `[build.download_window] text` | The line under the bar while the splash draws a first start's download: `{title}`, `{version}` (the app's), `{got}` and `{total}` (MB), `{pct}`. |
| `size` | `[width, height]` | the image's | The image box in px, 1 to 4096 each. |
| `fit` | `contain`, `cover` | `"contain"` | How the image fills `size` when its shape differs. |
| `text_position` | `below`, `over` | `"below"` | The text in a strip under the image, or over its lower part. |
| `align` | `left`, `center`, `right` | `"center"` | The text's alignment. |
| `margin` | px | 24 at the sides, 18 at the bottom | The text's gap to the edges. |
| `radius` | px | `0` | The box's corner radius (the window is clear around it). |
| `border` | `#rgb` / `#rrggbb` | none | A 1 px border. |
| `fade_ms` | ms | `100`; Android `200` | Its fade out (on the desktop where the OS fades windows: Windows, macOS). |
| `font` | family | the system's | The text's font: a generic family (`serif`, `sans-serif`, `monospace`; Android also `sans-serif-condensed`…) or a font's name. |
| `font_size`, `bold` | px (Android: points), bool | `13` (Android `14`), `false` | The text's size and weight. |
| `text_color` | `["#light", "#dark"]` | the app's | The text's colour. |
| `bar`, `track` | `["#light", "#dark"]` | the app's | The download bar's fill and ground, while the splash draws a first start's download. |
| `position` | `window`, `primary`, `pointer` | `"window"` | Where it opens: centred where the app's window will open (its last place), on the primary display, or on the one under the pointer. |
| `follow_window` | bool | `true` | With `position = "window"`: once the app's window shows, the splash moves onto where it really is. |
| `shadow` | bool | the OS's (macOS, Linux on; Windows off) | The text's shadow. |
| `window_shadow` | bool | the OS's (macOS on) | The window's drop shadow. |
| `cancel` | bool | `true` | The ✕ that cancels a first start's download. |
| `progress` | bool | `true` | The bar while a first start downloads Electron. |
| `bar_style` | `solid`, `thin` | `"solid"` | The bar's thickness: 6 or 3 points. |
| `bar_height` | px, 1 to 64 | `bar_style`'s | The bar's height. |
| `bar_position` | `under-text`, `bottom-edge` | `"under-text"` | Under the text inside the margins, or along the window's bottom edge, full width. |
| `bar_radius` | px | half its height | The bar's corner radius. |

```toml
[build.splash]
image = "assets/splash@2x.png"
text = "Starting {title}…"
background = ["#ffffff", "#101418"]
```

## [log]

The app, error and warning logs hold at most `3 × keep × file_mb`
together, whatever the app does ([debugging](../debugging/README.md)).

| Key | Type | Default | What it is |
|---|---|---|---|
| `level` | `trace`, `debug`, `info`, `warn`, `error` | `"info"` | The lowest level written. |
| `keep` | number | `5` | Files kept per log (`app.log`, `app.1.log`…), at least 1. |
| `file_mb` | number | `10` | A file rotates at this size, at least 1. |
| `heartbeat` | seconds | `3600` | Seconds between `heartbeat` lines (uptime, calls, errors); 0: none. |
| `suppress` | methods | `[]` | Methods (`"notes.tick"`) whose calls are not logged, even at `debug`: a method called many times a second. |
| `dir` | absolute or `~/…` | `<home>/logs` | The log folder of the app's own home (aio's `logging.dir`). A profile, an instance, `--home` or `AIORT_DATA` keeps its own `logs/`, so two copies never share a file. `ax logs` finds it from the app's workspace. Absolute as `[home] dir`. |
| `per_run` | bool | `false` | Each run starts fresh files (aio's per-run `debug.log`): the last run's move to `app.1.log`…, so `keep` runs are kept (a run past `file_mb` still rotates). |

```toml
[log]
level = "debug"
keep = 3
suppress = ["clock.tick"]
```

## [perf]

Time budgets in milliseconds (aio's `perfBudget`); a call or step over
its budget is a perf violation in the log and `ax health`.

| Key | Type | Default | What it is |
|---|---|---|---|
| `method` | milliseconds | 16 (debug), 100 (release) | A method's own time. |
| `effect` | milliseconds | `5` | One effect step. |
| `call_ceiling` | milliseconds | `30000` | How long each await of an async method may take (aio's `effectTimeoutMs`); one that never settles is cut off, its caller told. A `#[long]` method has none, a `#[long(ms)]` one its own. |
| `methods` | table | `{}` | Per method, `"cell.method" = ms`: one meant to be slow while every other stays strict. A key naming no method of its cell is reported (a typo would budget nothing). |
| `effects` | table | `{}` | Per effect, `"cell.name" = ms` (the name its key starts with). |

```toml
[perf]
method = 50

[perf.methods]
"notes.import" = 2000
```

### [perf.storm]

A method called `rate` times a second or more, `secs` seconds in a row, is
a storm (a feedback loop: a watcher that sees its own writes), said once
when it starts and once when it ends (aio's `dispatchStorm`).

| Key | Type | Default | What it is |
|---|---|---|---|
| `rate` | number | `200` | Calls per second. |
| `secs` | number | `5` | Seconds in a row. |
| `breaker` | bool | `false` | Refuse its calls while it lasts. |

## [memory]

When the process's memory is a problem, in percent of the machine's
(`ax health`, `ax heap`). Checked: `clear < high <= critical <= 100`, and
`growth_mb` at least 1, `every` 1 s to 10 s.

| Key | Type | Default | What it is |
|---|---|---|---|
| `high` | percent | `50` | High from this percent… |
| `clear` | percent | `40` | …and clear again below this one. |
| `critical` | percent | `80` | About to swap or be killed. |
| `growth_mb` | number | `256` | Growth said once per this many MB (or a tenth of the machine). |
| `every` | span | `"10s"` | How often memory is sampled and `health.json` written, 1 s to 10 s (aio's `memory.interval`). Each change of level reaches `aio::health::on_memory`. |

## [budgets]

Sizes and rates the app commits to. A breach is kept with its worst
reading, said once in the log and to the problem hook (`budget:<name>`),
and turns `/__aio/health` degraded, so a test or CI step can fail on it.
An app that declares none has no verdict.

| Key | Type | Default | What it is |
|---|---|---|---|
| `state` | size | none | Largest one cell's saved state may get (bytes of its JSON). |
| `payload` | size | none | Largest one push to one client. |
| `broadcast_rate` | rate | none | State pushes per second to one client. |

```toml
[budgets]
state = "1MB"
payload = "500KB"
broadcast_rate = "20/s"
```

## [feedback]

Problem reports, filed by the user (the `feedback` cell) or by the app
when it breaks. Always kept on disk first (`ax report <app>`).

| Key | Type | Default | What it is |
|---|---|---|---|
| `url` | URL | none | Each report is also POSTed here as JSON: `https://` only (`http://127.0.0.1:<port>` for tests). Its host must be in `[capabilities] network`. |
| `timeout` | span | `"15s"` | How long a POST may take before the report counts as not delivered. At least 1 s. |
| `keep` | number | `50` | Reports kept on disk, newest first. At least 1. |
| `auto` | bool | `true` | Report a problem by itself when the app breaks; `false`: only the user's reports. |
| `args` | bool | `false` | A report's timeline carries each call's arguments (aio's timeline payloads): a `#[redact]` method's as `[redacted]`, keys masked, 1 KB each. Off: none, since a report may leave the machine. |

## [ui]

The page's default look (aio's `ui.theme`, `ui.hue`, `neutral`, `ui.layout`, dark mode;
[theming](../ui/theming.md)); `aio::kit::theme::sheet` makes its CSS.

| Key | Type | Default | What it is |
|---|---|---|---|
| `theme` | `auto`, `full`, `tokens`, `none` | `"auto"` | How much of aio's look a page takes: `auto` (the whole look until the app ships a stylesheet of its own, then only the tokens), `full` (the whole look, the app's stylesheet over it), `tokens` (only the `--aio-*` tokens), `none` (no aio CSS: the kit is bare). |
| `scheme` | `system`, `light`, `dark` | `"system"` | Light or dark (`aio::ui::scheme()`, the template's `use_css!`): `system` follows the OS as it changes; the others fix one. |
| `hue` | 0 to 359 | from the id | The accent's hue in degrees, instead of the one hashed from the app's id. |
| `neutral` | `tinted`, `soft` | `"tinted"` | The page's greys (page, surfaces, borders): tinted with the hue, or near grey (`soft`, aio's `"neutral": "soft"`) for a warm or magenta hue; the accent keeps the hue. |
| `layout` | bool | `true` | With the look, the page layout: `<main>` as a centred page and the `.card`, `.row`, `.stack`, `.grid` classes. Off: only how elements look. |

```toml
[ui]
theme = "full"
hue = 210
neutral = "soft"
layout = false
```

## [vitals]

When a layer of the running app counts as degraded, warning or frozen
(`ax health`; aio's `vitals` and `renderBudget`). Each of
`[vitals.render]`, `[vitals.transport]`, `[vitals.loop]` and
`[vitals.queue]` takes all three of `degraded`, `warning`, `frozen` (ms;
for `queue`, effects waiting), and needs `degraded < warning < frozen`.

| Key | Default (`degraded`, `warning`, `frozen`) | What it is |
|---|---|---|
| `hints` | `true` | Say the likely cause with each warning and freeze. |
| `render` | 50, 200, 2000 | A render's time. |
| `transport` | 100, 500, 2000 | A round trip to the server. |
| `loop` | 100, 500, 2000 | The event loop's lag. |
| `queue` | 50, 200, 1000 | Effects waiting. |

```toml
[vitals.transport]
degraded = 250
warning = 1000
frozen = 5000
```

## [dev]

What `ax dev` does with the app.

| Key | Type | Default | What it is |
|---|---|---|---|
| `watch` | folders | `[]`: its sources and assets | Watch only these folders, relative to `aio.toml` (aio's `watch: [..]`): a change elsewhere neither patches nor rebuilds. `ax dev --watch=…` wins for one run. |

```toml
[dev]
watch = ["src/ui", "assets"]
```

## Options outside aio.toml

aio's options all go through `cell({...})` and `aio.run({...})`. In
aio-rs an option is in one of five places: a `#[cell(...)]` flag or a
field attribute, a method attribute inside `#[methods]`, a key of this
file, a call in the app's code (the launcher, an `Http` server, a
`Handle`), or a launch flag or variable. Some have no place because Rust
makes them unneeded; the rest are gaps, marked **gap**. The UI half of
aio's page (`aio/air`'s hooks and components) maps to `aio::ui` and the kit:
see [views](../ui/views.md) and the tables in
[migration](../migration/README.md#2-construct-by-construct).

### Cell options

| aio `cell(...)` | aio-rs |
|---|---|
| `state` | the struct's fields; `#[cell(custom_default)]` when it implements `Default` itself |
| `methods` | `#[methods] impl Cell { pub fn m(&mut self, …) -> Result }`; types in arguments are `#[aio::data]` |
| `scope: "client"` | a cell neither `persist` nor `shared`, in the view's module, or an `aio::ui::Global` |
| `cancelOn` | `cx.abort("key")` from the method that should cancel ([effects](../state/effects.md)) |
| `concurrency` | `#[mode(newest)]`, `#[mode(first)]` on a `pub async fn` (`first` joins a running call with the same arguments); per effect, `.mode(Mode::Newest / Queue / First)`; calls are always one at a time per cell |
| `ttl` | `#[ttl(ms)]` on a `pub async fn` |
| `long` | `#[long]` on a `pub async fn` |
| `selectors` | `#[derived] fn f(&self) -> T`; `#[derived(reads = [a, b])]` for a memo kept while those fields stay the same |
| `listensTo` | `target.listens_to(&source, "method", f)` on a `Handle` |
| `validate` | `#[invariant] fn …(&self) -> Result`, run after every method |
| `args` | the method's Rust types, checked as the call is decoded; finer rules with `ensure!` |
| `persist` | `#[cell(persist)]`; `#[transient]` on a field keeps it off disk |
| `diagnostics: false` | `#[cell(diagnostics = false)]` |
| `skipActions` | `#[skip_travel]` on a method: time travel steps over its events |
| `access` | `#[client]` on a method, `#[client(role = "admin")]`, `#[client(allow = f)]` |
| `visible` | `#[cell(visible = f)]`; `#[server]` on a field keeps it off the wire |
| `sync` | `#[cell(shared)]` |
| `worker`, `workerRespawn` | not needed: a method never blocks (I/O is an effect); heavy work runs in an effect off the cell's lock. No per-cell OS thread (**gap**) |
| `transaction` | `#[cell(transaction)]`, `transaction(serialize, conflict = "warn")`: an async method commits all or nothing (a plain call is atomic and runs on a copy anyway) |
| `version`, `onMigrate` | `#[cell(version = N, migrate = f)]` |
| `onRestore` | `#[cell(restore = f)]` (`fn(Self) -> Self`, every boot) |
| `onPersist` | `#[cell(save = f)]` (`fn(&Self) -> Self`) |
| `onInit`, `onDestroy` | `#[on_start]`, `#[on_stop]` on a `pub fn (&mut self)` method |
| a debounced write | `#[coalesce]` on a method, or `#[cell(coalesce)]` for all of them |
| state-machine guards | `aio::machine!` and `#[cell(machine = field)]` |
| an aio `db:` table | `#[table("name")]` on a field, `#[table("name", key = "col")]` on a map |
| deleting old cells | `#[cell(retire = ["old"])]` |
| schedules on a method | `#[every("5m")]`, `#[cron("0 9 * * 1")]`, `, skip_if_running` |
| a hook for aio messages | `#[on_aio]` (feature `compat`) |
| — | `#[cell(id = "todo")]`: the cell's id; unset, the type's name in snake case (`TodoList` is `todo_list`) |

### aio.run options

| aio `aio.run(...)` | aio-rs |
|---|---|
| `appId` | `[app] id` |
| `cells` | the handles the launcher opens (`aio::Source::for_app`, `Handle::open`), or one `aio::Cells` list |
| `localFirst` | built in: synced copies predict each call and replay offline calls on the server |
| `cellDefaults` | `persist`: `[data] only_persist = true` (aio's `persist: "none"` for every cell but `#[cell(persist)]` ones); `visible`: `aio::visible_by_default(f)`, for every cell without its own `visible = f` |
| `port`, `host`, `expose` | `[server] listen`; `--port`, `--host`, `--expose` per launch |
| `tls` | `[server] tls`, `cert`, `key` |
| `appDir` | `[home] dir`; per launch `--home`, `--profile`, `--instance`, `AIORT_DATA` |
| `profiles` | `[app] profiles` |
| `dbPath`, `dbPragmas` | `[data] store` (a file name in `data/`: the store is backed up, restored and checked by updates with the rest of the home; move the whole home with `[home] dir`), `[data] pragmas`; `":memory:"` is `[data] persist = false` or `--no-persist` |
| `checkIntegrityOnBoot` | always: `PRAGMA quick_check` and recovery at every open |
| `updates` | `[updates]` |
| `feedback` | `[feedback]` |
| `persist`, `persistKey`, `persistMode` | `#[cell(persist)]` per cell, `[data] persist = false` or `--no-persist` app-wide; each cell is its own row, keyed by its id |
| `persistDebounceMs` | `[data] coalesce_ms` for `#[coalesce]` runs (default `aio::PERSIST_DEBOUNCE`, 100 ms) |
| `ui` | `aio_shell::Window` in the launcher (title, size, icon, tray, head, lang, dir), `[app] lang`/`dir`, `[ui]` |
| `baseDir`, `watch` | `ax dev` watches the app's crate; `[dev] watch` (or `--watch=`) only some folders of it |
| `serveDirs`, `assets` | `[server.dirs]`, `[server.public_dirs]` |
| `client` | `[app] mode` and `[app] ui`; `--client=window\|browser\|server-only` per run |
| `keepServer` | `aio_shell::Window { keep_server }`, `--keep-server` per launch |
| `localConnectHooks` | `Http::door_links(true)`: `Http::on_link` hears the local door's links too (a peer app's copy; tools never) |
| `transport` | fixed: the window talks over a private socket, clients over `/ws` |
| `takeover` | `--takeover` |
| `serverUrl` | `[client] server`; `--server-url` per launch |
| `users`, `auth`, `sessions` | `[accounts]`, `Http::users`, `Http::oidc`, `Http::mail` |
| `key` | the app's key file and `AIORT_TOKEN`; `key: false` is `[server] open` |
| `pairing` | `[server] pairing` |
| `resolveUser` | `aio::auth::Users::resolve(f)` |
| `resolvePeer`, `peerTypes` | `Http::resolve_peer(f)`: the `Caller` (user, role) a peer app on this machine is; `aio::peer::Peer` calls one (`describe` lists what it may call); the contract is a crate both apps use, not text |
| `db` | `#[table(...)]` fields, `aio::db::Db` |
| `perfCheck`, `perfBudget` | `[perf]` |
| `budgets` | `[budgets]` |
| `renderBudget` | `[vitals.render]` |
| `wire` | none: the wire's encoding is fixed ([wire](../migration/wire.md)) |
| `effectTimeoutMs` | `[perf] call_ceiling` (ms, 30000 by default), the await ceiling of async methods; `#[long]` exempts one, `#[long(ms)]` gives one its own; per effect, `Effect::timeout(d)` |
| `freezeState` | not needed: a method gets `&mut` of a copy, the committed state is an `Arc` nobody can change |
| `memory` | `[memory]` |
| `circuitBreaker` | `aio::Cells::trip_after(max, window)` |
| `singleton` | always: a second launch hands over to the first; `--instance` for another |
| `strictCells` | not needed: a call names a typed handle, so a cell can't be left out unnoticed |
| `guardDispatches` | not needed: a refused call is a `Result`, a panic in a method refuses that call |
| `refusalsReject` | always: a refusal is the call's `Err` |
| `journal` | always: the state and its event commit in one transaction |
| `redactActions` | `#[redact]` on a method; `"vault:*"`: `#[cell(redact)]` |
| `childWindows`, `electron` | `aio_shell::Window { web, permissions, require_sandbox, show_cap_ms, … }`; `electron.downloadWindow` is `[build.download_window]`, `allowLocalPeers` `[server] local_peers`; `ignoreDebugEnv` is always on in a release build |
| `splash` | `[build.splash]`; its `show` at run time: `Window { splash_show }` |
| `libraryMode` | `Handle::memory()`, `Handle::open` without `aio_shell::start` (tests, embedding) |
| `syncIntervalMs`, `fullStateThreshold` | `[server.limits] sync_interval_ms`, `full_state_percent` (`0.5` is `50`) |
| `routes` | `aio::http::Http::route(Route::get(…))` |
| `maxConnections`, `wsLimits` | `[server.limits]` |
| `appFlags` | `[app] flags`: declared names; a near-miss is refused with the one meant, the rest reach the app |
| `allowedOrigins` | `[server] hosts` |
| `security` | `[server.security]`; compression with `Http::compress` |
| `plugins` | `aio::plugin::Plugin`, `aio::plugin::compose` |
| `strictOrigin`, `trustProxyHeader` | `[server] strict_origin`, `[server] trust_proxy` |
| `schedules` | `#[every]`, `#[cron]`, `cx.every`, `cx.cron`, `cx.after` |
| `isolate` | `AIORT_ISOLATE=todo,mail`: only those cells start, the others stay disabled |
| `beforeReduce` | `aio::before_call(f)` |
| `onAction` | `aio::on_action(f)`; one cell's: `Handle::listen(f)`, `on_change` |
| `onEffect` | `aio::on_effect(f)` |
| `onConnect`, `onDisconnect` | `Http::on_link(f)` (a `Presence`: online, user, address) |
| `onStart` | `#[on_start]` per cell, or code in the launcher before `aio_shell::run` |
| `fatalOnStart` | the launcher's `?`: an `Err` before `run` ends the process |
| `onStopping`, `onStop` | `aio::on_stopping(f)`, `aio::on_stop(f)` |
| `onError` | `aio::report::on_problem(f)` |
| `onRestore` | `#[cell(restore = f)]` |
| `logging` | `[log]`; `logging.dir` is `[log] dir` |
| `diagnostics` | always on; `#[cell(diagnostics = false)]` per cell |
| `dispatchStorm` | `[perf.storm]` |
| `onCheckpointRestore` | `#[cell(recovered = f)]` (`fn(Self, &aio::Recovered) -> Self`): the state a store back from a damaged file boots on; `aio::on_recovered(f)` hears each one; also said in the log and a report |

## Common mistakes

- `mode = "server"` without `[server] listen`: refused at build.
- `[updates] url` without `keys`: refused (no trust on first use); run
  `ax keygen <app>` and paste its line.
- A host in `[client] server`, `[updates] url` or `[feedback] url` that
  `[capabilities] network` leaves out: refused, naming the entry to add.
- `[home]` without `[app] port_of`: refused; `[home]` is only for ports.
- A relative `[server] cert` or `key`: refused. The file is built in, so
  give absolute paths, or set them on the machine that serves.
- Editing `aio.toml` and expecting a running or built app to see it:
  nothing is read at run time. Build again; per-launch changes are flags
  and variables.
- `"1h30m"` or `"1.5h"` for a span: refused. Write `"90m"`.
- `[perf.methods]` keyed by the method alone (`"import"`): it is
  `"cell.method"`.
- Changing `[app] id`, `[build.android] application_id` or
  `[build.macos] bundle_id` after a release: installs and data are found
  by them; the app starts as a new one.
- Secrets in `aio.toml`: it is built into the binary and usually
  committed. Keystore passwords, tokens and the sync key come from the
  environment or the app's key file ([accounts](../auth/README.md)).

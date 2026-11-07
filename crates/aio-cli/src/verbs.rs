//! The verb table (aio: `am-flags.ts` + `am-help-text.ts` in one): each
//! verb's usage line is the one source of its help, the flags it takes and
//! how many words it reads. One gate reads it before any verb runs: an
//! unknown or misplaced flag, a missing value or a word too many is refused
//! with the one right answer (exit 2), never dropped or read as a value.

/// One verb: `usage` after `ax ` (`<x>` needed, `[x]` optional, `…` any
/// number, `[--flag]` a switch, `[--flag v]` a flag with a value) and what it
/// does (lines after the first are indented under it).
pub struct Verb {
    pub name: &'static str,
    pub usage: &'static str,
    pub about: &'static str,
}

const fn v(name: &'static str, usage: &'static str, about: &'static str) -> Verb {
    Verb { name, usage, about }
}

pub const VERBS: &[Verb] = &[
    v(
        "create",
        "create <name> [--template t] [--id id] [--aio-version v] [--aio dir] [--target a,b] [--css tailwind] [--force] [--no-warm]",
        "a new app in ./<name> that runs, tests and builds as it is, on any
machine after a clone (its own workspace and git repository, aio.toml,
README, the agent brief's pointer); --template app (a window, the
default), cli (a command-line tool), service (served, no window), todo
(a window on a saved list) or canvas (a window drawing a world a pure,
tested function moves);
--id: its id (its data's home; default: the name, refused when an
installed app has it); --aio-version: its aio-rs release from git (default:
this ax's; main, latest); --aio: an aio-rs checkout instead, named relative
to the app; --target: its [build] targets (ax build --list); --css
tailwind: Tailwind writes its stylesheet; a failure leaves nothing behind;
at a terminal it then builds the app for ax dev in the background, so the
first ax dev is a launch (--no-warm, AIORT_NO_WARM=1: not)",
    ),
    v(
        "pin",
        "pin <app> [version] [--major]",
        "which aio-rs the app uses (aio's am pin): what its Cargo.toml files
ask for, what its Cargo.lock built, this ax's version and whether they
match, aio-rs's releases (exit 1: drift); version: every aio-rs
dependency moved to that release (0.3.0), main (the branch tip), latest
(the newest in the app's major; --major: any) or an aio-rs checkout's
folder (this machine only), Cargo.lock following; all or nothing; <app>:
its name or folder, left out inside it",
    ),
    v(
        "dev",
        "dev <app> [args…]",
        "run an app; edits reach it live (CSS, rsx! markup, Rust hot-patched), state
kept; what can't be patched rebuilds + restarts;
args go to the app (--instance <n>: a private copy; --fresh: on new data);
--display auto|isolated|current|:N: with no terminal (an agent) the window
goes on a nested display, not the person's, and opens no browser tab (auto);
--size 390x844: the window at that size for this run (its saved one untouched);
--watch=false (--no-watch): build and run it once; --watch=src/ui,assets:
reload, patch and rebuild on changes there only (dirs of the app);
--detach: the session in the background (logs/dev.out; ax stop ends it);
--warm: only the build ax dev starts from (ax create runs it in the background);
--warm --pack <dir>: instead, the prebuilt files of its libraries into
<dir>, from a build of their own (AIORT_PREBUILT_URL);
--verbose: the app's log at debug; --title <t>: the window's title this run;
--cdp[=port]: its window opens the DevTools protocol on loopback (a free
port, for other tools; ax shot, eval, surface and trigger need none);
--android [--device serial] [--native]: on the attached emulator or phone
(ANDROID_SERIAL; none: AIO_AVD booted): its web build in a dev APK installed
once beside the app, each edit reloading it (--native, or no web build: the
app itself for that CPU, again on every saved change); its log in the terminal;
--web [--port N] [--expose]: its web build (debug) served on loopback (a free
port unless --port; --expose: on the network over TLS, behind a key the
printed link carries), its tabs live: a saved stylesheet shows at once, any
other edit builds anew and the tabs reload",
    ),
    v(
        "keygen",
        "keygen <app> [--rotate] | keygen [--aio] (aio-rs's own key; --rotate too)",
        "the app's release signing key (in $XDG_CONFIG_HOME/aio-rt/keys, else ~/.config/aio-rt/keys) and its aio.toml lines;
--aio: aio-rs's own update key, which signs the SHA256SUMS of ax release --aio",
    ),
    v(
        "release",
        "release <app> [--channel c] [--notes t] [--key file] [--app-version v] [--out dir] [--windows] [--android] [--appimage] [--platforms a,b] [--patch-from versions] [--min-from v] [--commit] [--dir d] [--data file] [--no-build] [--allow-dirty] [--unsigned] [--workflow] | release [--aio] [--url u] [--target t] (aio-rs itself; --windows, --dir too)",
        "build, sign and lay out a release in dist/<channel>/ for [updates] url, with patches from earlier ones;
--platforms: one for each (as ax build; --windows: windows); --android: its signed APKs, offered to open on the device; --appimage: the AppImage, which an app run from one replaces itself with;
--min-from: installed only over v or newer (signed; an older install is told to install v first); --commit ([updates] sign_commit: always): the commit it was built from, signed in (--min-from and --commit need installs on aio-rs 0.3 or newer); --dir: in d/<channel>/; --data: what it reads, from a contract file, not asked of the build;
--out: the build's folder (as ax build --out), over [build] out;
--key: signed by that key file (its <name>.previous.key beside it too), not <keys>/<id>.key; --app-version: as that version (1.2.3), over [app] version and git;
--no-build: the bundles ax build left in dist/; --allow-dirty: a build of uncommitted edits too; --unsigned: signed by a key made for it alone, no ax keygen needed (to look at locally; no app takes it); --workflow: the GitHub workflow that does it on a tag;
--aio (in an aio-rs checkout): aio-rs itself prebuilt for this machine (--windows: Windows too; --target t: that triple too, e.g. x86_64-apple-darwin on an arm64 Mac) in dist/aio-rs/v<version>/ (--dir: in d), with install.sh / install.ps1 pinning each archive's SHA-256 (archives a Mac made, put in d first, too) and SHA256SUMS of every file in it; each signature made when its keys are set, else said unsigned (CONTRIBUTING.md: AIO_WINDOWS_CERT, AIO_MACOS_SIGN_IDENTITY, AIO_RELEASE_KEY or keygen --aio); --url: where they will be downloaded from (default: the GitHub release of github.com/riagentic/aio-rs tagged v<version>)",
    ),
    v(
        "update",
        "update <app> [--from-git <ref>]",
        "the installed app's versions and update state; --from-git: build branch or tag <ref> of its git source (or of its checkout) and stage it for its next start, UNSIGNED (a developer's path: never the update checks)",
    ),
    v("installed", "installed", "the apps installed from a bundle on this machine"),
    v(
        "remove",
        "remove <app> [--data]",
        "take an installed app away (not while it runs); its data stays, unless --data",
    ),
    v(
        "smoke",
        "smoke <artifact>",
        "start a window app's Linux bundle, .deb or AppImage headless as a user would,
then stop it: its boot time, its window's memory (PSS of the app and its
runtime), a clean stop, nothing left running (as ax build --smoke does);
a macOS app's zip on a Mac, a Windows bundle on Windows (AIO_WINDOWS_SSH)",
    ),
    v(
        "build",
        "build <app> [--targets a,b] [--platforms a,b] [--app-version v] [--out dir] [--list] [--sizes] [--smoke] [--smoke-strict] [--quick] [--deb] [--appimage] [--android] [--debug] [--windows] [--web] [--dev]",
        "the app's [build] targets (aio.toml), else an installable app for
this OS: dist/<id>-<version>-<os>-<arch>.* (Linux .tar.gz, macOS
zipped .app, Windows one-click .exe + zip; --windows: cross-built from Linux);
--targets: some of them, or kinds; --list: the kinds and targets;
--app-version: built as that version (1.2.3), over [app] version and git;
--out: into that folder, over [build] out (dist);
--platforms ([build] platforms): each desktop target for each of linux,
linux-arm64, windows, windows-arm64, macos, macos-arm64 or host; all:
every one, what this machine can't build said with why and left out;
--sizes: the release binary against its budget, packed nowhere, and which
crates take its space (over budget: always); size it so, not with a plain
`cargo build --release` (no ax link flags: some % bigger);
--smoke ([build] smoke: always): then start each Linux bundle, .deb
and AppImage headless, stop it, check its log; a macOS app's zip on
the Mac that built it (here, or [build.macos] host / AIO_MACOS_SSH); a
Windows bundle on Windows (AIO_WINDOWS_SSH: one over ssh), stopped with
<app> --quit; every other artifact listed as not smoke-tested, with why;
--smoke-strict: --smoke, and one not smoke-tested fails the build;
--quick: release builds without LTO, in 16 codegen units: a one-line
change rebuilds in seconds, the binary ~37% bigger; to try, never to ship
(--sizes and a Mac over ssh refuse it);
--deb: a Debian package ([app] maintainer); --appimage: one portable file;
--android: signed APKs instead (the app's lib calls android_app!; --debug:
debug APKs, the page in chrome://inspect);
--web: an installable web app (PWA) in dist/<id>-web/ instead: the app's
lib for wasm32, a manifest, Apple tags, icons and an offline service worker
(--web --dev: a debug build, its page showing its errors on it)",
    ),
    v(
        "lab",
        "lab <app> <os> [--stop] [--boot] [--status] [--reset] [--ram size] [--cpus n] [--disk size] [--os-version v] [--apk file] [--port n] [--dist dir] [--tunnel]",
        "the app on a real windows, macos, linux or android desktop, for a
person to click around in. A machine named in AIO_LAB_WINDOWS,
AIO_LAB_MACOS or AIO_LAB_LINUX ('[ssh options] [user@]host'): built for
it, copied over ssh into aio-rs-test/lab/<id> there and started with
its data, runtime and caches inside that folder; --stop: what runs from
it ended, the folder removed. Else (or --boot) aio's lab here, in Docker:
a Windows or macOS VM, an XFCE desktop or the Android emulator, its
viewer on a loopback port, the app's dist/ in it at /shared (android:
its APK installed once booted, --apk: which); --status, --stop, --reset
(a VM's disk too); a VM's --ram 8G --cpus 4 --disk 64G --os-version 11;
--port: the viewer's; --dist: that folder at /shared, not the app's dist/;
--tunnel: no port published, the viewer forwarded through docker exec by
this process until Ctrl-C (where Docker's published ports don't reach
this machine) (first Windows run: ~30 min, ~8.5 GB)",
    ),
    v(
        "install",
        "install <app> [--appimage] [--android] [--build] [--emulator] [--device serial] [--apk file] [--no-launch]",
        "build this OS's bundle from source and install it for this user (Linux: its
install.sh run, nothing left aside; macOS, Windows: how to); --appimage
(Linux): the AppImage, in the app menu, unpacked in a private TMPDIR; --android:
the newest APK for the attached phone's CPU onto it, then
start it; refuses an APK older than the sources (--build: build first);
--emulator: an emulator may be the device; --device (else ANDROID_SERIAL): that one",
    ),
    v(
        "lint",
        "lint [dir] [--json] [--no-hints] [--no-clippy] [--safe-fix] [--fix] [--from version] [--tests]",
        "what types can't check: credentials, impure methods, hooks,
a11y, cell ids, plus clippy (exit 1: errors; `ax lint --help`)",
    ),
    v(
        "test",
        "test <app> [filter] [--video out] [--pace ms] [--android] [--device serial] [--mutants]",
        "the app's tests (cargo test); --video records each UI test, no test
changed: out a folder (a video per test) or one .mp4/.webm, a frame per
step, each --pace ms (800) on screen; --android: the emulator lane instead (its
APK for the device built, installed, started: passed once its window is ready
and it still runs; AIO_AVD booted when none is attached, or --device);
--mutants: cargo mutants over the app's packages (each function broken in
turn: the ones no test noticed are listed; `cargo install cargo-mutants`)",
    ),
    v("apps", "apps", "the aio-rs apps and private instances on this machine, and which run"),
    v(
        "ui",
        "ui [--window] [--browser]",
        "the visual app manager: the apps on this machine, each
one's status, start and stop, memory and CPU, state, logs, metrics,
components and calls; opened detached (--window: here, until it closes;
--browser: in the browser, served from here on a loopback port behind a
key made for the run, until Ctrl-C)",
    ),
    v("open", "open <app> [--print]", "the running app's page in the browser, once it answers (--print: its address)"),
    v("data", "data <app>", "where the app keeps everything, and what to back up"),
    v("config", "config <app>", "the app's aio.toml as its build bakes it in, defaults in"),
    v(
        "events",
        "events <app> [cell] [--last N] [--diff] [--refused] [--from file]",
        "the newest events (default 20), oldest first; --diff: each with the
changes it made (one cell); --from: of another store or backup file;
--json of a running app: each with the time its method took (`ms`, this
run's), --refused: its refused calls of late too",
    ),
    v(
        "timeline",
        "timeline <app> [cell] [--last N] [--from file]",
        "what happened and what it changed: the newest events of every cell
(default 20), oldest first, each with its state diff (the running
app's, else its store's replayed by the app's code, built by cargo as
its tests are); --from: another store or backup file",
    ),
    v(
        "state",
        "state <app> <cell> [path] [--watch] [--as user]",
        "a cell's state, live from the running app (as user, default
you), else its newest stored snapshot; path: keys, indexes,
* (notes.*.text), filters (notes[?done=true].text, [?k], [?k!=v]),
picks (owner.{id,name}); --watch: a line now and after each change",
    ),
    v("snapshot", "snapshot <app> [--as user]", "every cell's state as one JSON object (live, else stored)"),
    v(
        "snapshot save",
        "snapshot save <app> [file] [--as user]",
        "the snapshot as a new file (default <id>-snapshot.json)",
    ),
    v(
        "snapshot load",
        "snapshot load <app> <file> [--force]",
        "put a snapshot file's cells in place on the running app, each as a
new event (ax timetravel goto steps back); refused when its cells
aren't the app's (another app's, an older build's) unless --force,
which loads the ones the app has; a snapshot taken --as someone
holds only what they saw",
    ),
    v(
        "replay",
        "replay <app> [range] [--from file] [--dry]",
        "call a log's inputs again on the running app, in order, to
reproduce a bug (range: N, N..M, N.., ..M; --from: another store or
backup); what they caused (schedules, effects) the app makes again;
--dry: what would be sent",
    ),
    v(
        "record",
        "record <app> [cell] <file.rs> [range] [--from file] [--type path]",
        "a Rust test from what the app's cell did (no cell: every cell the
range touched, their calls in the log's order): booted on the state before
the range, its calls made again, the state they left asserted; the
running app's, else (stopped, or --from) its store replayed by the
app's code, built by cargo as its tests are (--type: the cell's type,
default <crate>::<Cell>)",
    ),
    v(
        "persist",
        "persist <app> [--drop cell]",
        "write what the running app holds back now (coalesced calls, held
runs) instead of when it would; --drop: a stored cell this build no
longer declares, its history gone from the store (refused for a
cell the app declares)",
    ),
    v(
        "migrations",
        "migrations <app>",
        "each stored cell's version against the running app's, and its shape
drift: fields stored but no longer declared, or of another type",
    ),
    v(
        "call",
        "call <app> <cell> <method> [arg…] [--args json] [--body json] [--wait s] [--as user]",
        "call a method of the running app, print its answer; each arg
is JSON or text; k=v words (all of them): one object, its fields;
--args '[..]' exact (@file, - for stdin); --body '{..}': that one
object; --wait: its answer waited for s seconds at most (default:
until it comes)",
    ),
    v(
        "expect",
        "expect <app> <cell> <path> <op> [value] [--wait s] [--as user]",
        "assert on the live state (exit 1: it fails); op: eq ne gt gte lt
lte contains exists absent; --wait: until it passes, s seconds at most",
    ),
    v(
        "clients",
        "clients <app> [cell]",
        "the running app's windows (their --window number, title and page), or
the links open to its cell",
    ),
    v(
        "shot",
        "shot <app> [file] [--cdp port] [--window n] [--check base] [--update base] [--threshold n] [--max-diff r] [--selector css] [--full] [--size WxH] [--video file] [--duration s] [--markers]",
        "a PNG of the app's window (default <id>-<time>.png; --json: {file,
bytes, width, height}), over the DevTools
protocol (a dev build's own door: no flag; --cdp port, else AIORT_CDP,
else the port a launch with --cdp opened, when there is one);
--check: exit 1 unless it looks like base (a channel may move --threshold,
default 2; a share --max-diff of pixels may differ, default 0);
--update: record base; --selector: one element; --full: past the viewport;
--size 390x844: the page laid out at that size for the shot, then as it was;
--window: which of its windows (0, the first);
--video file.mp4|.webm: record the window until Ctrl-C (or --duration
seconds), encoded by ffmpeg (none: by the window itself, slower);
--markers: each ax trigger meanwhile in
file.markers.json, [{t, path, action, rect}] (ms, video px);
--json: {\"event\":\"recording\",\"file\",\"t0\"} on stderr once its first
frame is taken (t0: its epoch ms)",
    ),
    v(
        "eval",
        "eval <app> <code> [--cdp port] [--window n]",
        "JavaScript in the app's window, its value as JSON: an expression, or
statements and the last one's value ('const f = …; f(2)', top-level await),
as the DevTools console; promises awaited; a DOM node summarised; a throw
fails: geometry, computed styles, a fetch from the page's origin;
--window: which one (0, the first);
the window's DevTools protocol as for ax shot",
    ),
    v(
        "components",
        "components <app> [--renders] [--wait-client ms]",
        "the running app's mounted components, nested, each with its key,
how many times its body ran and how many times its holes wrote (aio
DevTools' tree and profile; a body runs once, a change runs only the holes
that read it); --renders: its last 200 renders as they happened, timed:
each body's run and each hole's rewrite, by component and key (aio
DevTools' render events); --json: all of it as JSON. A dev build's,
through its own door",
    ),
    v(
        "surface",
        "surface <app> [path] [--names] [--rects] [--component name] [--depth N] [--full] [--headless] [--root c] [--cdp port] [--window n] [--wait-client ms]",
        "what a person can see and use in the app's window, one line each,
nested: role \"name\" [handle] state, named as TestUi names them (a
data-t handle, else SaveButton, EmailInput, SaveButton2…); path: that
element and what is under it (row-2, list/row-3; none there: exit 1);
--component row: each element handled row or row-<n> (every
instance), with what is under it; --depth N: N levels down at most
(0: the elements alone); --full: names whole (else cut at 40);
--names: the auto names, in order;
--rects: where each is, @x,y wxh in CSS px; --wait-client: wait so long
for the window (right after a start or reload); --window, the protocol:
as ax eval; --headless: with no window open (a server), the app's views
(--root, default App) mounted headless on its running cells, built by
cargo as its tests are",
    ),
    v(
        "testgen",
        "testgen <app> [--out file] [--cdp port] [--window n] [--wait-client ms]",
        "a typed name table of what the running app's window shows, for its
UI tests: a const per handle and auto name (ui::SaveButton,
ui::row_2), so a renamed element fails to compile, not to run; written
to --out (default tests/ui_names/mod.rs; `mod ui_names;` in a test);
run it again after a UI change",
    ),
    v(
        "preview",
        "preview <app> <component> [props] [--html]",
        "one component of the app mounted alone with the props given, as Rust
fields ('name: \"Ada\".into(), times: 2'), and what it shows: its
surface, or its HTML; component: its path in the app's crate (ui::Card);
type-checked and rendered as the app's tests are, by cargo",
    ),
    v(
        "trigger",
        "trigger <app> <path> <action> [text] [--cdp port] [--window n] [--wait-client ms]",
        "act on the element at path as a person does, with real mouse and
keys: click dblclick hover focus blur, type <text> (appends), setValue
<text> (replaces), clear, press|keyDown|keyUp <key> (Enter, ctrl+k, mod+s),
select <value>, check uncheck, scroll <top[,left]>, dragTo <path>; path
window: a key to the window itself (a global key); --window: which
(default: the newest open); --wait-client, the protocol: as ax surface",
    ),
    v(
        "timetravel",
        "timetravel <app> <cell> [step] [seq]",
        "where the running cell is in its history, its newest calls and their
time; step: undo, redo, goto <seq> (the numbers ax events lists; each
pauses the cell: calls refused, schedules held), pause, resume. Every
step is a new event: none is lost",
    ),
    v(
        "doctor",
        "doctor <app> [--harden]",
        "is the running app the code on disk (version, build, edits since),
who decided each setting (config, dirs, environment), and hints: a
runtime older than this ax ships, an open door ([server] open);
--harden: the security switches it hasn't decided on written into its
aio.toml (a server beyond loopback: [server] strict_origin = true), said",
    ),
    v(
        "heap",
        "heap <app> [--watch]",
        "what the app holds: its memory, each cell's state and events;
--watch: again every 2 s (a top view; ax top: another interval)",
    ),
    v("top", "top <app> [secs]", "ax heap again every secs (2) seconds, until Ctrl-C; --json: once"),
    v("schedules", "schedules <app>", "saved schedules, soonest first"),
    v(
        "sql",
        "sql <app> [query…] [--tables]",
        "a read-only query of the store, as JSON rows (at most 10000);
--tables: the store's tables",
    ),
    v("tables", "tables <app>", "the store's tables"),
    v(
        "logs",
        "logs <app> [word] [--last N] [--level l] [--tag t] [--since s] [--client] [--follow]",
        "the newest log events (default 50), oldest first: those holding
word, at level l or worse, of category t (and t:*), newer than s
(15m, 2h, 2026-10-28T10:00); --client: the window's lines;
--follow (-f): then each new one; --json: counted",
    ),
    v(
        "metrics",
        "metrics <app> [--as user]",
        "uptime, open links, schedules and each cell's size (live when the
app runs, else stored)",
    ),
    v(
        "peer",
        "peer <app> [cell…] [--as user] [--role r] [--types file]",
        "what a caller (user, role) may call of the running app over the
network: each method's Rust signature and what its #[client] rule says
(cells: those, default all); app: one here, or another machine's by its
profile (a .aioapp, ax profile's file; its cells named), asked as that
profile's key holder; --types: written as Rust consts (a module a cell,
a const a method), so a call by a renamed name fails to compile (file -:
on stdout)",
    ),
    v(
        "cost",
        "cost <app> [cell] [--keys] [--window s]",
        "what each cell of the running app sent its clients (pushes, bytes per
second, mean, full resends) and its methods' p95 time; its top keys by
bytes (--keys: all of them); --window 5m: over the newest 5 minutes
(seconds, or 30s, 5m, 1h; default: since its first send)",
    ),
    v(
        "errors",
        "errors <app> [--last N] [--since s]",
        "ax dev's failed build, then the newest warnings and errors
(default 20), each under the start of the app it came from;
--since: newer than s (15m, 2h, 2026-10-28T10:00), reload: since its
newest start or hot patch",
    ),
    v("health", "health <app>", "healthy, or why not (exit 1: degraded, stuck or not running)"),
    v("vitals", "vitals <app>", "each layer's grade (loop, queue, transport, render) and the likely cause"),
    v(
        "status",
        "status <app> [app…]",
        "started (exit 0), starting, stopping, maintenance (exit 2), stopped
(exit 1; a crash or kill says so); one line an app",
    ),
    v(
        "stop",
        "stop <app> [app…] [--all] [--wait s] [--no-wait]",
        "ask the app to close (any OS) and wait until it is gone; still there
after s (15) seconds: SIGTERM, then SIGKILL; --no-wait: asked, not
waited for; --all: every app of this workspace; an --instance started
--fresh: its data thrown away once it is gone",
    ),
    v(
        "start",
        "start <app> [arg…] [--wait s] [--no-wait] [--fresh] [--display d] [--cdp] [--all]",
        "run the app in the background (built first in its workspace, else
the installed one) and wait until it is up, at most s (30) seconds
(--no-wait: until it is spawned); args
after -- go to the app; its output: logs/start.log; --fresh, --display,
--cdp: as ax dev; --all: every app of this workspace (its components)",
    ),
    v(
        "restart",
        "restart <app> [arg…] [--wait s] [--cdp] [--all]",
        "stop it if it runs, then start it again with the flags it was
started with (a flag given here replaces its recorded one); --all:
every app of this workspace",
    ),
    v("kill", "kill <app>", "end the app now, no asking (its pid from its health.json)"),
    v("report", "report <app> [id]", "the app's problem reports, newest first; one in full"),
    v(
        "setup",
        "setup [--check] [--warm] [--aio dir]",
        "what making apps needs: checks Rust and the C toolchain, fetches the
shared runtime and wasm-bindgen (the web build's) prebuilt and checked, and
lists the optional tools; each missing one with the command that gets it;
exit 1 while a needed one is missing; --check: look only, fetch nothing;
--warm: then builds aio-rs and its dependencies as ax dev does, once for the
machine (on --aio's checkout, else this ax's release), so a new app's first
ax dev builds only its own crates",
    ),
    v(
        "upgrade",
        "upgrade [version] [--aio dir]",
        "ax itself to aio-rs's newest release (or version), by the release's
install script, checked against its SHA256SUMS (it checks the archive's
SHA-256 before anything runs); --aio: ax built from that checkout instead
(cargo install; plain ax upgrade returns to a release); apps keep their
own aio-rs (ax pin)",
    ),
    v(
        "uninstall",
        "uninstall [--tools]",
        "ax out of cargo's bin folder; your apps, their data and installs stay;
--tools: also what ax fetched and built in the tool home (tools/, its
build folder target/)",
    ),
    v("runtime", "runtime [--fetch]", "where the shared runtime is (fetch it if missing); --prune: ax prune"),
    v(
        "pair",
        "pair <app> [host:port] [code] [--plain]",
        "a fresh pairing code from the running app (aio's am pair; 8 digits,
6 with --plain; valid 3 minutes, once); with host:port and code: pair
this device with that app's server by its code",
    ),
    v(
        "prune",
        "prune [--yes] [--days N] [--keep a,b]",
        "what of the shared runtime store may go (also: ax runtime --prune):
what no installed app names and nothing started from in N (30) days;
the one this ax ships, --keep ones and unknown ones stay; --yes (-y)
deletes what the plan names",
    ),
    v(
        "theme adopt",
        "theme adopt <app> [--force]",
        "aio-rs's theme into the app as assets/aio-theme.css, its own file from
then on (no upgrade moves its look); --force: write over it",
    ),
    v(
        "trust",
        "trust [path]",
        "this machine's root, and how to make browsers trust it (it explains;
it installs nothing); path: the file alone",
    ),
    v(
        "auth",
        "auth <app> <do> [id] [value] [--role r] [--email e] [--password p]",
        "the app's accounts, running or not: users · create <id> · passwd <id>
· role <id> <role> · totp <id> off · unlock <id> · verify <id> · revoke <id>
· rm <id> (no --password: one is made and shown once)",
    ),
    v(
        "profile",
        "profile <app> [--out file] [--link]",
        "the running app's profile (.aioapp): host, port, root, key: a
credential (--out writes it 0600); hand it to a client. --link: a browser
link carrying the key (?token=)",
    ),
    v("import", "import <app> <file>", "link this device to an app by its profile, as pair does"),
    v("backup", "backup <app> [file]", "copy the app's data (safe while it runs)"),
    v("restore", "restore <app> <file>", "put a backup back (the app must be closed; the data it replaces is kept)"),
    v(
        "discover",
        "discover [--timeout ms]",
        "the aio-rs apps on this LAN that answer ([server] discover = true), within ms (1200)",
    ),
    v("feedback", "feedback [app] [--create]", "where findings about aio-rs go; one app's file (--create: make it)"),
    v("agent", "agent [section] [--list] [--min] [--max]", "the brief for agents: the rules first, then how aio-rs works"),
    v(
        "control",
        "control <app> [--port n]",
        "the running app's verbs over loopback HTTP, for tools that can't run
ax (aio's /__aio/trojan/*): GET /__aio/trojan/<verb>?<arg>&<flag>=v
answers what ax <verb> <app> <arg> --<flag> v --json does, behind a
key made for the run (Authorization: Bearer <key>, printed first);
until Ctrl-C",
    ),
    v("help", "help [verb] [--commands] [--all]", "the everyday verbs; --commands (--all): every verb; verb: that one in full"),
    v("version", "version", "the aio-rs version of this ax"),
];

/// Verbs that moved or aren't there, and what to do instead.
const MOVED: &[(&str, &str)] = &[
    ("dispatch", "ax calls a method with `ax call <app> <cell> <method> [arg…]`"),
    (
        "fix",
        "a clone builds as it is (aio-rs from git); ax pin <version> moves an app's aio-rs, ax pin <dir> to a checkout",
    ),
    ("prune versions", "ax has no version store to prune: an app pins aio-rs in its Cargo.lock"),
    ("theme", "ax theme adopt <app> [--force]: aio-rs's theme into the app, as its own file"),
    ("instances", "ax apps: every app and private instance on this machine, and which run"),
    ("watch", "ax dev <app>: it runs with each edit live (--watch=src/ui,assets: only those)"),
    ("actions", "ax events <app> [cell]: the newest events; ax timeline <app>: each with its diff"),
    ("client", "ax components <app>: the window's component tree (ax surface <app>: what it shows)"),
    ("compile", "ax build <app> (--targets a,b: some of its [build] targets)"),
    ("publish", "ax release <app> [--channel c] [--dir d] [--notes t] [--key file] [--app-version v]"),
    ("migrate", "ax lint <app> --from <version> [--tests]: what a newer aio-rs retired that the app still uses"),
    ("check", "cargo check: rustc checks client and server code in one pass; ax lint <app>: aio-rs's checks too"),
    ("where", "the compiler decides: rustc resolves client and server code, cargo check names a misplaced call"),
    ("add", "ax has no generators: a cell is a struct with #[aio::cell] (ax create <name> makes a new app)"),
    ("link", "ax pin <app> <dir>: the app's aio-rs from that checkout (ax pin <app> <version>: a release)"),
];

/// Other spellings of a verb: these words, then this flag, read as that
/// verb (`ax runtime --prune --yes` is `ax prune --yes`).
const ALIASES: &[(&str, &str, &str)] = &[("runtime", "--prune", "prune")];

/// A verb's one-dash short flags, and the flag each stands for.
const SHORT: &[(&str, &str, &str)] = &[("logs", "-f", "--follow"), ("prune", "-y", "--yes")];

/// A verb that names no app when given this flag (`stop --all`).
const NO_APP_WITH: &[(&str, &str)] =
    &[("stop", "--all"), ("start", "--all"), ("restart", "--all"), ("release", "--aio"), ("keygen", "--aio")];

/// Flags every verb takes.
const GLOBAL: &[&str] = &[
    "--help",
    "--version",
    "--json",
    "--quiet",
    "--app <name>",
    "--profile <name>",
    "--instance <name>",
    "--home <dir>",
];

/// What `ax help` shows (aio's `EVERYDAY`): what it takes to get from
/// nothing to a running, inspected, driven app; the rest is one
/// `--commands` away.
const EVERYDAY: &[(&str, &str)] = &[
    ("agent", "the brief for agents: the rules, then how aio-rs works"),
    ("create", "a new app in ./<name> that runs, tests and builds as it is"),
    ("dev", "run an app; edits hot-patch in, else rebuild + restart"),
    ("test", "the app's tests (cargo test), --video to watch them"),
    ("start", "run it in the background and wait until it is up"),
    ("stop", "ask it to close and wait until it is gone"),
    ("restart", "stop it, start it again with the flags it had"),
    ("status", "started, starting, stopping or stopped (exit 0, 2, 1)"),
    ("logs", "the newest log events; --follow for new ones"),
    ("errors", "the failed build, then the newest warnings and errors"),
    ("state", "a cell's state, live (else stored); --watch for changes"),
    ("call", "call a method of the running app"),
    ("surface", "what a person can see and use in the window"),
    ("trigger", "click, type, press… on an element, as a person"),
    ("shot", "a PNG (or --video) of the window; --check a baseline"),
    ("timeline", "the newest events of every cell, each with its diff"),
    ("doctor", "is the running app the code on disk; who set what"),
    ("build", "an installable app for this OS (or the [build] targets)"),
    ("release", "build, sign and lay out a release for updates"),
];

/// Flags that pick where the app runs (its home), and the verbs about an
/// app that take none: they work on its source, not a running copy.
const HOMED: [&str; 4] = ["--app", "--profile", "--instance", "--home"];
const UNHOMED: [&str; 10] =
    ["dev", "keygen", "release", "update", "build", "config", "remove", "install", "theme adopt", "pin"];

const FOOT: &str = "Inside an app's folder <app> may be left out; --app <name> names it anywhere.
<app>@<profile>, --profile, --instance, --home: that copy of the app (as its launch takes them);
asked for none, and the app runs from exactly one other home, the verbs follow it.
--json: one JSON object on stdout ({\"ok\":…}); --quiet: nothing on stdout but failures;
exit 0 ok, 1 failed, 2 bad usage.
AIORT_DATA=<dir> points at one app's dirs instead of the tool home.";

pub fn find(name: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|v| v.name == name)
}

/// What one usage line says.
#[derive(Debug, PartialEq)]
pub struct Shape {
    /// Flag names, and whether each takes a value.
    pub flags: Vec<(&'static str, bool)>,
    /// The words: needed, optional, and whether any number may follow.
    pub need: usize,
    pub optional: usize,
    pub any: bool,
    /// Whether the first word is `<app>`.
    pub app: bool,
}

pub fn shape(usage: &'static str) -> Shape {
    let mut s = Shape { flags: vec![], need: 0, optional: 0, any: false, app: false };
    // After the verb's own name (`snapshot save`).
    let mut words = usage.split(' ').skip_while(|w| !w.starts_with(['<', '['])).peekable();
    while let Some(w) = words.next() {
        if let Some(f) = w.strip_prefix("[--") {
            let name = &w[1..w.len() - usize::from(f.ends_with(']'))];
            let valued = !f.ends_with(']');
            if valued {
                words.next();
            }
            s.flags.push((name, valued));
        } else if w.contains('…') {
            s.any = true;
        } else if w.starts_with('<') {
            s.app |= s.need == 0 && s.optional == 0 && w == "<app>";
            s.need += 1;
        } else {
            s.optional += 1;
        }
    }
    s
}

fn global(name: &str) -> Option<bool> {
    GLOBAL.iter().find(|g| g.split(' ').next() == Some(name)).map(|g| g.contains(' '))
}

/// What the command line asks for.
#[derive(Debug, PartialEq)]
pub enum Ask {
    /// Print this (help, version).
    Say(String),
    /// Run the verb with these args: `--x=v` split, `--app` and the app of
    /// the folder put in as the first word.
    Run(Vec<String>),
}

fn version() -> String {
    format!("aio-rs {}", env!("CARGO_PKG_VERSION"))
}

/// The everyday verbs, one line each: one screen.
pub fn help() -> String {
    let mut out = String::from("ax — the aio-rs command\n\n");
    for (name, about) in EVERYDAY {
        out.push_str(&format!("  ax {name:<10}{about}\n"));
    }
    out.push_str("\nax help --commands: every verb; ax help <verb>: one in full.\n");
    out.push_str(FOOT);
    out
}

/// Every verb, each with all it says.
pub fn commands() -> String {
    let mut out = String::from("ax — the aio-rs command\n\n");
    for v in VERBS {
        let usage = format!("ax {}", v.usage);
        let about: Vec<&str> = v.about.lines().collect();
        let rest = if usage.len() < 34 {
            out.push_str(&format!("  {usage:<34}{}\n", about[0]));
            &about[1..]
        } else {
            out.push_str(&format!("  {usage}\n"));
            &about[..]
        };
        for l in rest {
            out.push_str(&format!("{:36}{l}\n", ""));
        }
    }
    out.push('\n');
    out.push_str(FOOT);
    out
}

/// One verb in full.
pub fn verb_help(v: &Verb) -> String {
    let s = shape(v.usage);
    let own: Vec<_> = s.flags.iter().map(|(f, _)| *f).collect();
    let takes = match (own.is_empty(), s.any) {
        // Its flags are among its args (`dev <app> [args…]`): those its text names.
        (true, true) => "the flags named above (the rest of its args go on as they are)".into(),
        (true, false) => "no flags of its own".into(),
        _ => own.join(" "),
    };
    format!(
        "ax {}\n\n{}\n\n{} takes {takes}; every verb also: {}\n{FOOT}",
        v.usage,
        v.about.lines().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n"),
        v.name,
        GLOBAL.join(" ")
    )
}

/// Read the command line: help and version wherever asked, every flag
/// known to its verb, every word counted. `here` names the app of the
/// folder (asked only when `<app>` is left out). `Err` is bad usage.
pub fn read(args: &[String], here: &dyn Fn() -> Result<String, String>) -> Result<Ask, String> {
    if let Some(first) = args.first()
        && let Some((_, flag, to)) = ALIASES.iter().find(|(v, f, _)| v == first && args.iter().any(|a| a == f))
    {
        let rest = args[1..].iter().filter(|a| a != flag).cloned();
        return read(&std::iter::once(to.to_string()).chain(rest).collect::<Vec<_>>(), here);
    }
    // A two-word verb (`snapshot save`) reads as one word.
    if let [first, second, rest @ ..] = args {
        let two = format!("{first} {second}");
        if find(&two).is_some() || MOVED.iter().any(|(m, _)| *m == two) {
            return read_one(&[vec![two], rest.to_vec()].concat(), here);
        }
    }
    read_one(args, here)
}

fn read_one(args: &[String], here: &dyn Fn() -> Result<String, String>) -> Result<Ask, String> {
    let Some(first) = args.first() else { return Ok(Ask::Say(help())) };
    let first = first.as_str();
    // Up to a bare `--`; after it every word is a word.
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let asked = |f: &[&str]| args[..end].iter().any(|a| f.contains(&a.as_str()));
    if let Some((_, to)) = MOVED.iter().find(|(m, _)| *m == first) {
        return Err((*to).into());
    }
    if matches!(first, "--version" | "-V") || (asked(&["--version", "-V"]) && first != "dev") {
        return Ok(Ask::Say(version()));
    }
    if matches!(first, "--help" | "-h") {
        return Ok(Ask::Say(help()));
    }
    let Some(verb) = find(first) else {
        let hint = aio::cli::near(first, VERBS.iter().map(|v| v.name))
            .map_or(String::new(), |n| format!(" (did you mean {n}?)"));
        return Err(format!("unknown command: {first}{hint}\n  ax help lists them"));
    };
    let see = format!("\n  ax {}\n  ax help {}", verb.usage, verb.name);
    // `dev` hands what follows the app to the app; `lint` reads its own.
    let own = match verb.name {
        "dev" => args.len().min(2),
        "lint" => return Ok(Ask::Run(args.to_vec())),
        _ => end,
    };
    if args[1..own].iter().any(|a| a == "--help" || a == "-h") {
        return Ok(Ask::Say(verb_help(verb)));
    }
    if verb.name == "help" {
        let every = args[1..own].iter().any(|a| a == "--commands" || a == "--all");
        return match args.get(1) {
            None => Ok(Ask::Say(help())),
            Some(_) if every && own == 2 => Ok(Ask::Say(commands())),
            Some(_) => {
                let name = args[1..own].join(" ");
                find(&name).map(|v| Ask::Say(verb_help(v))).ok_or_else(|| format!("no verb `{name}`"))
            }
        };
    }
    let s = shape(verb.usage);
    let mut out = vec![first.to_string()];
    let mut words: Vec<String> = vec![];
    let mut app: Option<String> = None;
    let mut i = 1;
    while i < own {
        // A short flag reads as the one it stands for.
        let a = &SHORT
            .iter()
            .find(|(v, f, _)| *v == verb.name && *f == args[i])
            .map_or_else(|| args[i].clone(), |(.., l)| l.to_string());
        i += 1;
        let (name, inline) = a.split_once('=').map_or((a.as_str(), None), |(f, v)| (f, Some(v.to_string())));
        if !name.starts_with("--") {
            // `-x` is a flag nobody has (but `-3` and `-` are words).
            if verb.name != "call"
                && a.len() > 1
                && a.starts_with('-')
                && a[1..].starts_with(|c: char| c.is_ascii_alphabetic())
            {
                return Err(format!("unknown flag {a}: ax's flags are --words{see}"));
            }
            words.push(a.clone());
            continue;
        }
        let valued = match s.flags.iter().find(|(f, _)| *f == name) {
            Some((_, valued)) => *valued,
            None => match global(name) {
                Some(valued) => valued,
                None => {
                    let pool = s.flags.iter().map(|(f, _)| *f).chain(GLOBAL.iter().filter_map(|g| g.split(' ').next()));
                    let hint = aio::cli::near(name, pool).map_or(String::new(), |n| format!(" (did you mean {n}?)"));
                    let own: Vec<_> = s.flags.iter().map(|(f, _)| *f).collect();
                    let takes = if own.is_empty() { "no flags of its own".into() } else { own.join(" ") };
                    return Err(format!(
                        "{}: unknown flag {name}{hint}\n  {} takes {takes}{see}",
                        verb.name, verb.name
                    ));
                }
            },
        };
        let value = match (valued, inline) {
            (false, Some(_)) => return Err(format!("{name} takes no value{see}")),
            (false, None) => None,
            (true, Some(v)) => Some(v),
            (true, None) if i < own => {
                i += 1;
                Some(args[i - 1].clone())
            }
            (true, None) => return Err(format!("{name} needs a value{see}")),
        };
        if HOMED.contains(&name) && (!s.app || (UNHOMED.contains(&verb.name) && name != "--app")) {
            return Err(format!("{} is about no running app: {name} does nothing for it{see}", verb.name));
        }
        match (name, value) {
            ("--app", Some(v)) => app = Some(v),
            (_, v) => out.extend(std::iter::once(name.to_string()).chain(v)),
        }
    }
    let rest = if verb.name == "dev" { &args[own..] } else { args.get(own + 1..).unwrap_or_default() };
    words.extend(rest.iter().cloned());
    let alone = NO_APP_WITH.iter().any(|(v, f)| *v == verb.name && out.iter().any(|o| o == f)) && words.is_empty();
    if s.app && !alone {
        match app {
            Some(a) => words.insert(0, a),
            None if words.len() < s.need => words.insert(0, here()?),
            None => {}
        }
    }
    if words.len() < s.need && !alone {
        return Err(format!("{}: too few words{see}", verb.name));
    }
    if !s.any && words.len() > s.need + s.optional {
        let extra = words[s.need + s.optional..].join(" ");
        return Err(format!("{}: one word too many: {extra}{see}", verb.name));
    }
    if verb.name == "version" {
        return Ok(Ask::Say(version()));
    }
    // Words after the flags, behind `--` so none is read as a flag again.
    out.push("--".into());
    out.extend(words);
    Ok(Ask::Run(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(line: &str) -> Result<Ask, String> {
        let args: Vec<String> = line.split(' ').filter(|w| !w.is_empty()).map(String::from).collect();
        read(&args, &|| Ok("here-app".into()))
    }

    fn run(line: &str) -> Vec<String> {
        match r(line) {
            Ok(Ask::Run(a)) => a,
            other => panic!("{line}: {other:?}"),
        }
    }

    fn said(line: &str) -> String {
        match r(line) {
            Ok(Ask::Say(s)) => s,
            other => panic!("{line}: {other:?}"),
        }
    }

    #[test]
    fn a_usage_line_says_flags_and_words() {
        let s = shape("build <app> [--targets a,b] [--list] [--smoke]");
        assert_eq!(s.flags, vec![("--targets", true), ("--list", false), ("--smoke", false)]);
        assert_eq!((s.need, s.optional, s.any, s.app), (1, 0, false, true));
        let s = shape("call <app> <cell> <method> [arg…] [--args json] [--as user]");
        assert_eq!((s.need, s.optional, s.any), (3, 0, true));
        assert_eq!(shape("events <app> [cell] [--last N]").optional, 1);
        assert!(!shape("pair x <host:port>").app && !shape("runtime [--fetch]").app);
        // Every verb's line reads: each flag once, no flag a global one.
        for v in VERBS.iter().filter(|v| v.name != "lint") {
            let f = shape(v.usage).flags;
            assert!(f.iter().all(|(n, _)| n.starts_with("--") && global(n).is_none()), "{}", v.usage);
            assert!(v.usage == v.name || v.usage.starts_with(&format!("{} ", v.name)), "{}", v.name);
        }
    }

    #[test]
    fn an_unknown_flag_is_refused_with_what_was_meant() {
        // aio am-unknown-flags: every gated verb refuses --zzz.
        for v in VERBS.iter().filter(|v| !["dev", "lint", "help", "version"].contains(&v.name)) {
            let e = r(&format!("{} a b c d --zzz", v.name)).unwrap_err();
            assert!(e.contains("unknown flag --zzz"), "{}: {e}", v.name);
        }
        let e = r("events app --lats 3").unwrap_err();
        assert!(e.contains("unknown flag --lats (did you mean --last?)") && e.contains("events takes --last"), "{e}");
        assert!(r("health app --watch").unwrap_err().contains("health takes no flags of its own"));
        let dev = verb_help(find("dev").unwrap());
        assert!(dev.contains("--display") && !dev.contains("no flags of its own"), "its flags are in its args: {dev}");
        // A one-dash flag is a typo too; a negative number is a word.
        assert!(r("state app notes -w").unwrap_err().contains("unknown flag -w"));
        assert_eq!(run("call app c m -3 -x"), ["call", "--", "app", "c", "m", "-3", "-x"]);
        // Unknown command: did you mean.
        assert!(r("evnts app").unwrap_err().contains("did you mean events?"));
        assert!(r("dispatch notes:set x").unwrap_err().contains("ax call"));
    }

    #[test]
    fn an_aio_verb_typed_from_habit_names_its_ax_form() {
        // Every `am` verb ax hasn't: refused, naming the exact ax verb.
        for (am, ax) in [
            ("instances", "apps"),
            ("watch", "dev"),
            ("actions", "events"),
            ("client", "components"),
            ("compile", "build"),
            ("publish", "release"),
            ("migrate", "lint"),
            ("check", "lint"),
            ("add", "create"),
            ("link", "pin"),
            ("dispatch", "call"),
            ("fix", "pin"),
            ("theme", "theme adopt"),
        ] {
            let e = r(&format!("{am} app")).unwrap_err();
            assert!(e.contains(&format!("ax {ax}")) && find(ax).is_some(), "{am}: {e}");
        }
        assert!(r("where src/x.rs").unwrap_err().contains("cargo check"));
        assert!(r("publish app").unwrap_err().starts_with("ax release <app>"));
    }

    #[test]
    fn values_words_and_the_end_of_flags() {
        assert_eq!(run("events app --last=5"), ["events", "--last", "5", "--", "app"]);
        assert_eq!(run("events app --last 5 notes"), ["events", "--last", "5", "--", "app", "notes"]);
        assert_eq!(run("timetravel app board goto 3"), ["timetravel", "--", "app", "board", "goto", "3"]);
        assert!(r("events app --last").unwrap_err().contains("--last needs a value"));
        assert!(r("health app --json=1").unwrap_err().contains("--json takes no value"));
        assert!(r("health app extra").unwrap_err().contains("one word too many: extra"));
        assert!(r("smoke").unwrap_err().contains("too few words"));
        assert!(r("pair app host code extra").unwrap_err().contains("one word too many: extra"));
        // After `--`, a word that looks like a flag is a word.
        assert_eq!(run("call app c m -- --x"), ["call", "--", "app", "c", "m", "--x"]);
        assert_eq!(run("health app --json"), ["health", "--json", "--", "app"]);
    }

    #[test]
    fn the_app_comes_from_app_or_the_folder() {
        assert_eq!(run("health"), ["health", "--", "here-app"]);
        assert_eq!(run("events --app notes cell"), ["events", "--", "notes", "cell"]);
        // Given as a word, the folder isn't asked.
        let args: Vec<String> = ["health", "mine"].map(String::from).to_vec();
        assert_eq!(
            read(&args, &|| Err("not asked".into())),
            Ok(Ask::Run(vec!["health".into(), "--".into(), "mine".into()]))
        );
        let none = read(&["logs".to_string()], &|| Err("no app here: give one".into()));
        assert_eq!(none, Err("no app here: give one".into()));
        assert!(r("apps --app x").unwrap_err().contains("--app does nothing"));
        // --all: every app of the workspace, no folder asked.
        assert_eq!(run("start --all"), ["start", "--all", "--"]);
        assert_eq!(run("restart --all --wait 5"), ["restart", "--all", "--wait", "5", "--"]);
        assert_eq!(run("logs notes --instance a1"), ["logs", "--instance", "a1", "--", "notes"]);
        assert!(r("build notes --profile p").unwrap_err().contains("--profile does nothing"));
        assert_eq!(run("build --app notes"), ["build", "--", "notes"]);
    }

    #[test]
    fn help_and_version_on_any_verb() {
        // aio's tiers: the everyday verbs on one screen, every verb a flag away.
        let day = said("help");
        assert!(day.lines().count() <= 30 && day.contains("  ax state     a cell's state"), "{day}");
        assert!(EVERYDAY.iter().all(|(v, _)| find(v).is_some()), "an everyday verb is a verb");
        assert_eq!(said(""), day);
        assert_eq!(said("--help"), day);
        let all = said("help --commands");
        assert_eq!(said("help --all"), all, "aio's spelling");
        assert!(all.contains("ax state <app> <cell> [path] [--watch] [--as user]"), "{all}");
        assert!(all.lines().any(|l| l.starts_with("  ax data <app>") && l.ends_with("and what to back up")));
        let one = said("events --help");
        assert!(
            one.starts_with(
                "ax events <app> [cell] [--last N] [--diff] [--refused] [--from file]\n\n  the newest events"
            ),
            "{one}"
        );
        assert!(one.contains("events takes --last"), "{one}");
        assert_eq!(said("help events"), one);
        assert_eq!(said("state -h"), said("help state"));
        assert!(r("help nope").is_err());
        let ver = format!("aio-rs {}", env!("CARGO_PKG_VERSION"));
        assert_eq!(said("--version"), ver);
        assert_eq!(said("logs app --version"), ver);
        assert_eq!(said("version"), ver);
        // `dev` hands the app's own flags on.
        assert_eq!(run("dev app --help --x"), ["dev", "--", "app", "--help", "--x"]);
        assert!(said("dev --help").starts_with("ax dev"));
    }

    #[test]
    fn the_new_flags_read_as_their_verbs_take_them() {
        assert_eq!(run("health app --quiet"), ["health", "--quiet", "--", "app"]);
        assert_eq!(run("timeline app --last 5"), ["timeline", "--last", "5", "--", "app"]);
        assert_eq!(run("stop app --no-wait"), ["stop", "--no-wait", "--", "app"]);
        assert_eq!(run("top app 5"), ["top", "--", "app", "5"]);
        assert_eq!(run("call app c m title=x --wait 2"), ["call", "--wait", "2", "--", "app", "c", "m", "title=x"]);
        assert_eq!(
            run("surface app --component row --depth 1 --full")[..5],
            ["surface", "--component", "row", "--depth", "1"]
        );
        assert!(r("lab app linux --version 2").is_ok_and(|a| matches!(a, Ask::Say(_))), "--version stays ax's own");
        assert_eq!(run("lab app windows --boot --os-version 10")[..4], ["lab", "--boot", "--os-version", "10"]);
    }

    #[test]
    fn another_spelling_reads_as_its_verb() {
        assert_eq!(run("runtime --prune --yes --days 3"), run("prune --yes --days 3"));
        assert_eq!(run("runtime --prune"), ["prune", "--"]);
        assert_eq!(run("runtime --fetch"), ["runtime", "--fetch", "--"]);
        assert!(r("runtime --prune --fetch").unwrap_err().contains("prune: unknown flag --fetch"));
        assert!(said("help --commands").contains("also: ax runtime --prune"));
    }
}
